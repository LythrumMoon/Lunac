// 调音（v1 `player.rs` 的控制面）的 Dart 薄层。
//
// 边界：链的合法性、频响曲线、预设文件、A/B 快照全在 Rust（`rust/src/tuning.rs` +
// `tuning-engine/`）；这里只做 JSON 解码与容错，**不重算任何滤波器/曲线**。
//
// 纪律（与 `platform/` 其它文件一致）：桥接不可用 / 坏 JSON ⇒ 明确状态或一句人话，不抛异常。
// 这些调用都是「读一个小 JSON + 建一条链（几十个双二阶）」级别，不需要 `Isolate.run`。
import 'dart:convert';
import 'dart:ffi';
import 'dart:isolate';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';

/// 一段滤波器（`on=false` 的段保留在配置里、不参与建链）。
class TuningFilter {
  const TuningFilter({
    required this.kind,
    required this.freqHz,
    this.gainDb,
    this.q,
    this.on = true,
  });

  final String kind;
  final double freqHz;
  final double? gainDb;
  final double? q;
  final bool on;

  static TuningFilter fromJson(Map<String, dynamic> json) => TuningFilter(
    kind: _str(json['kind']),
    freqHz: _double(json['freq_hz']),
    gainDb: _optDouble(json['gain_db']),
    q: _optDouble(json['q']),
    on: json['on'] != false,
  );

  Map<String, Object?> toJson() => {
    'kind': kind,
    'freq_hz': freqHz,
    if (gainDb != null) 'gain_db': gainDb,
    if (q != null) 'q': q,
    'on': on,
  };

  /// 只改其中几项（调音页的频段柱 / 曲线拖动都走它）。
  TuningFilter copyWith({
    String? kind,
    double? freqHz,
    double? gainDb,
    double? q,
    bool? on,
  }) => TuningFilter(
    kind: kind ?? this.kind,
    freqHz: freqHz ?? this.freqHz,
    gainDb: gainDb ?? this.gainDb,
    q: q ?? this.q,
    on: on ?? this.on,
  );
}

/// 一条声道复制（引擎 `channel_copy`）。
class TuningCopy {
  const TuningCopy({required this.from, required this.to});

  final int from;
  final int to;

  static TuningCopy fromJson(Map<String, dynamic> json) =>
      TuningCopy(from: _int(json['from']), to: _int(json['to']));

  Map<String, Object?> toJson() => {'from': from, 'to': to};
}

/// 一条通道段（引擎 `if_else`）。
class TuningCondBlock {
  const TuningCondBlock({
    required this.channel,
    required this.thenFilters,
    required this.elseFilters,
  });

  final int channel;
  final List<TuningFilter> thenFilters;
  final List<TuningFilter> elseFilters;

  static TuningCondBlock fromJson(Map<String, dynamic> json) => TuningCondBlock(
    channel: _int(json['channel']),
    thenFilters: _filters(json['then']),
    elseFilters: _filters(json['else']),
  );

  Map<String, Object?> toJson() => {
    'channel': channel,
    'then': thenFilters.map((f) => f.toJson()).toList(),
    'else': elseFilters.map((f) => f.toJson()).toList(),
  };
}

/// 一种滤波器类型（引擎 `Kind::ALL`，面板照它画下拉）。
class TuningKind {
  const TuningKind({
    required this.name,
    required this.usesGain,
    required this.order,
  });

  final String name;
  final bool usesGain;
  final int order;

  static TuningKind fromJson(Map<String, dynamic> json) => TuningKind(
    name: _str(json['name']),
    usesGain: json['gain'] == true,
    order: _int(json['order']),
  );
}

/// 频响曲线（20Hz–20kHz，240 点；`db` 与 `freqs` 一一对应）。
class TuningResponse {
  const TuningResponse({
    required this.sampleRate,
    required this.freqs,
    required this.db,
  });

  final double sampleRate;
  final List<double> freqs;
  final List<double> db;

  static const empty = TuningResponse(sampleRate: 48000, freqs: [], db: []);

  static TuningResponse fromJson(Map<String, dynamic> json) => TuningResponse(
    sampleRate: _double(json['sampleRate']),
    freqs: _doubles(json['freqs']),
    db: _doubles(json['db']),
  );
}

/// 完整调音状态（`lunac_tuning_get` 的 `data`）。
class TuningState {
  const TuningState({
    required this.enabled,
    required this.preset,
    required this.presets,
    required this.userPresets,
    required this.kinds,
    required this.preampDb,
    required this.bassDb,
    required this.trebleDb,
    required this.filters,
    required this.graphicEq,
    required this.delayMs,
    required this.channelCopy,
    required this.convolution,
    required this.ifElse,
    required this.channels,
    required this.condDb,
    required this.response,
  });

  final bool enabled;
  final String preset;
  final List<String> presets;
  final List<String> userPresets;
  final List<TuningKind> kinds;
  final double preampDb;
  final double bassDb;
  final double trebleDb;
  final List<TuningFilter> filters;
  final List<Object?> graphicEq;
  final double delayMs;
  final List<TuningCopy> channelCopy;
  final String? convolution;
  final List<TuningCondBlock> ifElse;

  /// 最近一次播放那条流的声道数（`0` = 还不知道，面板按 2 兜底）。
  final int channels;
  final List<List<double>> condDb;
  final TuningResponse response;

  static const empty = TuningState(
    enabled: false,
    preset: 'flat',
    presets: [],
    userPresets: [],
    kinds: [],
    preampDb: 0,
    bassDb: 0,
    trebleDb: 0,
    filters: [],
    graphicEq: [],
    delayMs: 0,
    channelCopy: [],
    convolution: null,
    ifElse: [],
    channels: 0,
    condDb: [],
    response: TuningResponse.empty,
  );

  static TuningState fromJson(Map<String, dynamic> json) => TuningState(
    enabled: json['enabled'] == true,
    preset: _str(json['preset']),
    presets: _strings(json['presets']),
    userPresets: _strings(json['userPresets']),
    kinds: _objects(json['kinds'])
        .map(TuningKind.fromJson)
        .toList(growable: false),
    preampDb: _double(json['preampDb']),
    bassDb: _double(json['bassDb']),
    trebleDb: _double(json['trebleDb']),
    filters: _filters(json['filters']),
    graphicEq: (json['graphicEq'] is List)
        ? List<Object?>.from(json['graphicEq'] as List)
        : const [],
    delayMs: _double(json['delayMs']),
    channelCopy: _objects(
      json['channelCopy'],
    ).map(TuningCopy.fromJson).toList(growable: false),
    convolution: (json['convolution'] is String &&
            (json['convolution'] as String).isNotEmpty)
        ? json['convolution'] as String
        : null,
    ifElse: _objects(
      json['ifElse'],
    ).map(TuningCondBlock.fromJson).toList(growable: false),
    channels: _int(json['channels']),
    condDb: (json['condDb'] is List)
        ? (json['condDb'] as List)
              .whereType<List>()
              .map((row) => row.map(_double).toList(growable: false))
              .toList(growable: false)
        : const [],
    response: json['response'] is Map<String, dynamic>
        ? TuningResponse.fromJson(json['response'] as Map<String, dynamic>)
        : TuningResponse.empty,
  );
}

/// 一次调音调用的结果：`ok=false` 时 `error` 是**人话**（`ERR_*` 机器码也在里面）。
class TuningReply {
  const TuningReply({required this.ok, required this.error, this.state});

  final bool ok;
  final String error;
  final TuningState? state;

  static const unavailable = TuningReply(ok: false, error: '桥接不可用');
}

/// 读调音状态（含频响曲线）。
TuningReply readTuning() => _decode(_callNoArg('lunac_tuning_get'));

/// 只改总开关（不重置链）。
TuningReply setTuningEnabled(bool enabled) => _decode(
  _callInt('lunac_tuning_set_enabled', enabled ? 1 : 0),
);

/// 整条链重置成某条预设（内置 / 用户预设都认）。
TuningReply setTuningPreset({required bool enabled, required String preset}) =>
    _decode(_callIntText('lunac_tuning_set_preset', enabled ? 1 : 0, preset));

/// 把一条预设整条展开进来（开关不动）。
TuningReply applyTuningPreset(String preset) =>
    _decode(_callText('lunac_tuning_apply_preset', preset));

/// 覆盖整条链（`preset` 变 `custom`）。
TuningReply setTuningChain({
  required double preampDb,
  required List<TuningFilter> filters,
  double bassDb = 0,
  double trebleDb = 0,
  List<Object?> graphicEq = const [],
  double delayMs = 0,
  List<TuningCopy> channelCopy = const [],
  String? convolution,
  List<TuningCondBlock> ifElse = const [],
}) {
  final payload = jsonEncode({
    'preampDb': preampDb,
    'bassDb': bassDb,
    'trebleDb': trebleDb,
    'filters': filters.map((f) => f.toJson()).toList(),
    'graphicEq': graphicEq,
    'delayMs': delayMs,
    'channelCopy': channelCopy.map((c) => c.toJson()).toList(),
    'convolution': convolution ?? '',
    'ifElse': ifElse.map((b) => b.toJson()).toList(),
  });
  return _decode(_callText('lunac_tuning_set_chain', payload));
}

/// 把当前链存成用户预设（`overwrite=false` 时同名会失败并提示）。
TuningReply saveTuningPreset(String name, {bool overwrite = false}) => _decode(
  _callTextInt('lunac_tuning_preset_save', name, overwrite ? 1 : 0),
);

TuningReply deleteTuningPreset(String name) =>
    _decode(_callText('lunac_tuning_preset_delete', name));

TuningReply renameTuningPreset(String from, String to) =>
    _decode(_callTextText('lunac_tuning_preset_rename', from, to));

/// 导入预设文件（**导入即应用**）。
TuningReply importTuningPreset(String path) =>
    _decode(_callText('lunac_tuning_preset_import', path));

/// 导出当前链到预设文件。
TuningReply exportTuningPreset(String path) =>
    _decode(_callText('lunac_tuning_preset_export', path));

/// 弹系统「打开」选一个预设文件；取消 / 桥接不可用 ⇒ null。
String? pickTuningPresetFile() => _pickPath('lunac_tuning_preset_pick');

/// 弹系统「另存为」选导出位置；取消 / 桥接不可用 ⇒ null。
String? pickTuningExportPath() => _pickPath('lunac_tuning_preset_pick_save');

/// 弹系统「打开」选卷积脉冲响应（WAV）；取消 ⇒ null。
String? pickTuningIrFile() => _pickPath('lunac_tuning_pick_ir');

/// 三个选文件入口的同一套解析：只认 `{ok:true, path}`，取消不是错误。
String? _pickPath(String symbol) {
  final raw = _callNoArg(symbol);
  if (raw == null) {
    return null;
  }
  try {
    final value = jsonDecode(raw);
    if (value is Map<String, dynamic> &&
        value['ok'] == true &&
        value['path'] is String) {
      final path = (value['path'] as String).trim();
      return path.isEmpty ? null : path;
    }
  } catch (_) {
    // 坏 JSON ⇒ 当作取消。
  }
  return null;
}

/// 一次**实时测量**的结果（`lunac_tuning_measure`）。
///
/// 三条曲线的下标都对着 [freqs]（长度可能少于 240 —— 扫频两端 / 能量过低的点会被摘掉）：
///   · [db]       = 最终输出（链 × 系统）
///   · [chainDb]  = **仅自身链**（这条才该跟合成曲线对齐）
///   · [refDb]    = 同频点上的链合成（宿主解析式）
class TuningMeasurement {
  const TuningMeasurement({
    required this.sampleRate,
    required this.freqs,
    required this.db,
    required this.chainDb,
    required this.refDb,
    required this.coverage,
    required this.elapsedMs,
  });

  final double sampleRate;
  final List<double> freqs;
  final List<double> db;
  final List<double> chainDb;
  final List<double> refDb;

  /// 有效率（拿到的点 / 240）。低于 ~0.7 说明这次没覆盖住整个频段。
  final double coverage;
  final int elapsedMs;

  static TuningMeasurement? fromJson(Object? value) {
    if (value is! Map) {
      return null;
    }
    final freqs = _doubles(value['freqs']);
    if (freqs.isEmpty) {
      return null;
    }
    return TuningMeasurement(
      sampleRate: _double(value['sampleRate']),
      freqs: freqs,
      db: _doubles(value['db']),
      chainDb: _doubles(value['chainDb']),
      refDb: _doubles(value['refDb']),
      coverage: _double(value['coverage']),
      elapsedMs: _int(value['elapsedMs']),
    );
  }
}

/// 跑一次实时测量（WASAPI 回环，**约 6 秒、会出声**）。
///
/// 两趟各约 2 秒的阻塞采集 ⇒ 放 `Isolate.run`，别冻住 UI 帧。
/// 失败时 `error` 是**一句人话**（`ERR_NO_AUDIO_DEVICE` / 设备格式不匹配等）。
Future<({bool ok, String error, TuningMeasurement? data})> measureTuning() async {
  String? raw;
  try {
    raw = await Isolate.run(_measureInIsolate);
  } catch (_) {
    raw = null;
  }
  if (raw == null) {
    return (ok: false, error: '桥接不可用', data: null);
  }
  try {
    final value = jsonDecode(raw);
    if (value is! Map<String, dynamic>) {
      return (ok: false, error: '返回格式异常', data: null);
    }
    return (
      ok: value['ok'] == true,
      error: value['error'] is String ? value['error'] as String : '',
      data: TuningMeasurement.fromJson(value['data']),
    );
  } catch (_) {
    return (ok: false, error: '返回格式异常', data: null);
  }
}

String? _measureInIsolate() {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_NoArgC, _NoArgDart>('lunac_tuning_measure')();
    return raw == nullptr ? null : raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    _free(library, raw);
  }
}

/// A / B 两槽各自有没有快照。
({bool a, bool b}) tuningAbState() {
  final raw = _callNoArg('lunac_tuning_ab_state');
  if (raw == null) {
    return (a: false, b: false);
  }
  try {
    final value = jsonDecode(raw);
    // 这一条的 `data` 是 `{a, b}`，不是 `TuningState` ⇒ 单独解析。
    if (value is Map<String, dynamic> && value['data'] is Map<String, dynamic>) {
      final data = value['data'] as Map<String, dynamic>;
      return (a: data['a'] == true, b: data['b'] == true);
    }
  } catch (_) {
    // 坏 JSON ⇒ 当作两槽都空（面板据此置灰按钮）。
  }
  return (a: false, b: false);
}

/// 把当前链快照进 A / B 槽。
TuningReply saveTuningAb(String slot) =>
    _decode(_callText('lunac_tuning_ab_save', slot));

/// 应用 A / B 槽的快照。
TuningReply applyTuningAb(String slot) =>
    _decode(_callText('lunac_tuning_ab_apply', slot));

// ── FFI 细节 ─────────────────────────────────────────────────────────

typedef _NoArgC = Pointer<Utf8> Function();
typedef _NoArgDart = Pointer<Utf8> Function();
typedef _IntC = Pointer<Utf8> Function(Int32);
typedef _IntDart = Pointer<Utf8> Function(int);
typedef _TextC = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _TextDart = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _IntTextC = Pointer<Utf8> Function(Int32, Pointer<Utf8>);
typedef _IntTextDart = Pointer<Utf8> Function(int, Pointer<Utf8>);
typedef _TextIntC = Pointer<Utf8> Function(Pointer<Utf8>, Int32);
typedef _TextIntDart = Pointer<Utf8> Function(Pointer<Utf8>, int);
typedef _TextTextC = Pointer<Utf8> Function(Pointer<Utf8>, Pointer<Utf8>);
typedef _TextTextDart = Pointer<Utf8> Function(Pointer<Utf8>, Pointer<Utf8>);
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

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

String? _callInt(String symbol, int value) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_IntC, _IntDart>(symbol)(value);
    return raw == nullptr ? null : raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    _free(library, raw);
  }
}

String? _callText(String symbol, String text) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? input;
  Pointer<Utf8>? raw;
  try {
    input = text.toNativeUtf8();
    raw = library.lookupFunction<_TextC, _TextDart>(symbol)(input);
    return raw == nullptr ? null : raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    if (input != null) {
      malloc.free(input);
    }
    _free(library, raw);
  }
}

String? _callIntText(String symbol, int value, String text) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? input;
  Pointer<Utf8>? raw;
  try {
    input = text.toNativeUtf8();
    raw = library.lookupFunction<_IntTextC, _IntTextDart>(symbol)(value, input);
    return raw == nullptr ? null : raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    if (input != null) {
      malloc.free(input);
    }
    _free(library, raw);
  }
}

String? _callTextInt(String symbol, String text, int value) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? input;
  Pointer<Utf8>? raw;
  try {
    input = text.toNativeUtf8();
    raw = library.lookupFunction<_TextIntC, _TextIntDart>(symbol)(input, value);
    return raw == nullptr ? null : raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    if (input != null) {
      malloc.free(input);
    }
    _free(library, raw);
  }
}

String? _callTextText(String symbol, String a, String b) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? first;
  Pointer<Utf8>? second;
  Pointer<Utf8>? raw;
  try {
    first = a.toNativeUtf8();
    second = b.toNativeUtf8();
    raw = library.lookupFunction<_TextTextC, _TextTextDart>(symbol)(first, second);
    return raw == nullptr ? null : raw.toDartString();
  } catch (_) {
    return null;
  } finally {
    if (first != null) {
      malloc.free(first);
    }
    if (second != null) {
      malloc.free(second);
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
    // 符号缺失时忽略。
  }
}

TuningReply _decode(String? raw) {
  if (raw == null) {
    return TuningReply.unavailable;
  }
  try {
    final value = jsonDecode(raw);
    if (value is! Map<String, dynamic>) {
      return const TuningReply(ok: false, error: '返回格式异常');
    }
    final ok = value['ok'] == true;
    final data = value['data'];
    return TuningReply(
      ok: ok,
      error: _str(value['error']),
      state: data is Map<String, dynamic> ? TuningState.fromJson(data) : null,
    );
  } catch (_) {
    return const TuningReply(ok: false, error: '返回格式异常');
  }
}

String _str(Object? value) => value is String ? value : '';

int _int(Object? value) => value is int ? value : (value is num ? value.toInt() : 0);

double _double(Object? value) => value is num ? value.toDouble() : 0;

double? _optDouble(Object? value) => value is num ? value.toDouble() : null;

List<double> _doubles(Object? value) =>
    value is List ? value.whereType<num>().map((n) => n.toDouble()).toList(growable: false) : const [];

List<String> _strings(Object? value) =>
    value is List ? value.whereType<String>().toList(growable: false) : const [];

List<Map<String, dynamic>> _objects(Object? value) => value is List
    ? value.whereType<Map<String, dynamic>>().toList(growable: false)
    : const [];

List<TuningFilter> _filters(Object? value) => _objects(value)
    .map(TuningFilter.fromJson)
    .toList(growable: false);
