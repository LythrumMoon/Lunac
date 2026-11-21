// 平台薄层：桌宠（v1 `app/src/plugins/builtin/pet.ts` 的窗口 / 形象那一半）。
//
// 边界：窗口、WebView2、配置读写、模型校验都在 `rust/src/pet.rs`；本文件只包
// `lunac_pet_*` 这组 C ABI 并做容错解析。
//
// 配置口径（与 v1 一致）：
//   · v1 存同源 localStorage 的 `lunac.pet.v1`；v2 没有同源 localStorage（桌宠窗在
//     原生 WebView2 里），落到 `<exe 根>\ModuleData\pet\lunac.pet.v1.json`，**字段名与
//     键名都沿用 v1**（image / model / scale / offsetX / offsetY / rotation /
//     clickThrough / winW / winH …），v1 的其它字段原样保留。
//
// 纪律：桥接缺符号 / 坏 JSON 时返回默认值 + 一句人话，不抛异常、不崩面板。
import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import 'native_bridge.dart';

typedef _StringC = Pointer<Utf8> Function();
typedef _StringDart = Pointer<Utf8> Function();
typedef _StrArgC = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _StrArgDart = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _TwoStrArgC = Pointer<Utf8> Function(Pointer<Utf8>, Pointer<Utf8>);
typedef _TwoStrArgDart = Pointer<Utf8> Function(Pointer<Utf8>, Pointer<Utf8>);
typedef _ThreeStrArgC = Pointer<Utf8> Function(
  Pointer<Utf8>,
  Pointer<Utf8>,
  Pointer<Utf8>,
);
typedef _ThreeStrArgDart = Pointer<Utf8> Function(
  Pointer<Utf8>,
  Pointer<Utf8>,
  Pointer<Utf8>,
);
typedef _BoolArgC = Pointer<Utf8> Function(Bool);
typedef _BoolArgDart = Pointer<Utf8> Function(bool);
typedef _UintArgC = Pointer<Utf8> Function(Uint32);
typedef _UintArgDart = Pointer<Utf8> Function(int);
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

/// 桌宠配置（字段名与 v1 `PetConfig` 一致；只列 v2 用到的那些）。
class PetConfig {
  const PetConfig({
    this.image = '',
    this.model = '',
    this.scale = 1,
    this.offsetX = 0,
    this.offsetY = 0,
    this.rotation = 0,
    this.clickThrough = false,
    this.topmost = true,
    this.visible = true,
    this.winW = 320,
    this.winH = 510,
    this.expressions = const {},
    this.params = const {},
    this.proactive = const ProactiveConfig(),
    this.motionTemplateIncludePhysics = false,
  });

  final String image;
  final String model;
  final double scale;
  final double offsetX;
  final double offsetY;
  final double rotation;
  final bool clickThrough;
  final bool topmost;
  final bool visible;
  final double winW;
  final double winH;

  /// 各模型当前生效的表情名（模型路径 → 表情名；v1 同款键）。
  final Map<String, String> expressions;

  /// 各模型调试参数的持久化值（模型路径 → {参数 id → 数值}）。
  final Map<String, Map<String, double>> params;

  /// 「主动反应」配置（v1 `ProactiveCfg`，含今日计数）。
  final ProactiveConfig proactive;

  /// 新建动作模板是否包含**物理输出参数**（v1 同名配置；默认 false）。
  final bool motionTemplateIncludePhysics;

  /// 当前形象（`model` 优先，与 v1 `loadConfig` 同口径）。
  bool get isLive2D => model.trim().isNotEmpty;
  String get shapePath => isLive2D ? model : image;

  PetConfig copyWith({
    String? image,
    String? model,
    double? scale,
    double? rotation,
    bool? clickThrough,
    bool? topmost,
    bool? visible,
  }) {
    return PetConfig(
      image: image ?? this.image,
      model: model ?? this.model,
      scale: scale ?? this.scale,
      offsetX: offsetX,
      offsetY: offsetY,
      rotation: rotation ?? this.rotation,
      clickThrough: clickThrough ?? this.clickThrough,
      topmost: topmost ?? this.topmost,
      visible: visible ?? this.visible,
      winW: winW,
      winH: winH,
      expressions: expressions,
      params: params,
      proactive: proactive,
      motionTemplateIncludePhysics: motionTemplateIncludePhysics,
    );
  }

  static PetConfig fromJson(Object? json) {
    if (json is! Map) {
      return const PetConfig();
    }
    double number(String key, double fallback) {
      final value = json[key];
      return value is num ? value.toDouble() : fallback;
    }

    String text(String key) {
      final value = json[key];
      return value is String ? value : '';
    }

    Map<String, String> expressions() {
      final raw = json['expressions'];
      if (raw is! Map) {
        return const {};
      }
      final out = <String, String>{};
      raw.forEach((key, value) {
        if (key is String &&
            key.trim().isNotEmpty &&
            value is String &&
            value.trim().isNotEmpty) {
          out[key] = value;
        }
      });
      return out;
    }

    Map<String, Map<String, double>> params() {
      final raw = json['params'];
      if (raw is! Map) {
        return const {};
      }
      final out = <String, Map<String, double>>{};
      raw.forEach((model, values) {
        if (model is! String || values is! Map) {
          return;
        }
        final one = <String, double>{};
        values.forEach((id, value) {
          if (id is String && value is num) {
            one[id] = value.toDouble();
          }
        });
        if (one.isNotEmpty) {
          out[model] = one;
        }
      });
      return out;
    }

    return PetConfig(
      image: text('image'),
      model: text('model'),
      scale: number('scale', 1),
      offsetX: number('offsetX', 0),
      offsetY: number('offsetY', 0),
      rotation: number('rotation', 0),
      clickThrough: json['clickThrough'] == true,
      topmost: json['topmost'] != false,
      visible: json['visible'] != false,
      winW: number('winW', 320),
      winH: number('winH', 510),
      expressions: expressions(),
      params: params(),
      proactive: ProactiveConfig.fromJson(json['proactive']),
      motionTemplateIncludePhysics:
          json['motionTemplateIncludePhysics'] == true,
    );
  }
}

/// 「主动反应」配置（v1 `ProactiveCfg`；计数字段 `date` / `used` 与配置共用同一个对象）。
///
/// 读「今日已用」的口径与 v1 一致：`date != 今天` ⇒ 视为 0（跨日自动归零，不必写盘）。
/// 这里再夹一次区间，是因为面板的输入框允许人键入任意数字；宿主也会夹（两处同口径）。
class ProactiveConfig {
  const ProactiveConfig({
    this.enabled = false,
    this.intervalMin = 30,
    this.dailyCap = 20,
    this.screenPollSec = 20,
    this.screenMinGapSec = 600,
    this.screenDiff = 12,
    this.date = '',
    this.used = 0,
  });

  /// **默认关**：这条链路持续烧 token，必须用户显式同意才开。
  final bool enabled;

  /// 两次主动反应之间的最小间隔（分钟，1..1440，默认 30）。
  final int intervalMin;

  /// 每个自然日最多触发几次（0..200；**0 = 不限**）。
  final int dailyCap;

  /// 屏幕指纹轮询间隔（秒，5..600，默认 20）。
  final int screenPollSec;

  /// 屏幕反应最小间隔（秒，30..3600，默认 600）。
  final int screenMinGapSec;

  /// 屏幕反应灵敏度（16×9 灰度平均绝对差阈值，1..64，默认 12；越小越敏感）。
  final int screenDiff;

  /// 「今日已用」计数对应的自然日（本地 `YYYY-MM-DD`）。
  final String date;

  /// 「今日已用」次数（`date` 不是今天时读出来按 0 算）。
  final int used;

  static int _clamp(Object? value, int min, int max, int fallback) {
    if (value is! num || !value.isFinite) {
      return fallback;
    }
    return value.round().clamp(min, max);
  }

  static ProactiveConfig fromJson(Object? json) {
    if (json is! Map) {
      return const ProactiveConfig();
    }
    final date = json['date'];
    final used = json['used'];
    return ProactiveConfig(
      enabled: json['enabled'] == true,
      intervalMin: _clamp(json['intervalMin'], 1, 1440, 30),
      dailyCap: _clamp(json['dailyCap'], 0, 200, 20),
      screenPollSec: _clamp(json['screenPollSec'], 5, 600, 20),
      screenMinGapSec: _clamp(json['screenMinGapSec'], 30, 3600, 600),
      screenDiff: _clamp(json['screenDiff'], 1, 64, 12),
      date: date is String ? date : '',
      used: used is num && used.isFinite && used > 0 ? used.floor() : 0,
    );
  }

  /// 只带「用户可编辑」的字段 —— 计数不参与保存（避免面板把 today 计数写回）。
  Map<String, Object?> toPatch() => {
    'enabled': enabled,
    'intervalMin': intervalMin,
    'dailyCap': dailyCap,
    'screenPollSec': screenPollSec,
    'screenMinGapSec': screenMinGapSec,
    'screenDiff': screenDiff,
  };

  /// 本地自然日（`YYYY-MM-DD`）。
  static String todayKey([DateTime? now]) {
    final date = now ?? DateTime.now();
    final month = date.month.toString().padLeft(2, '0');
    final day = date.day.toString().padLeft(2, '0');
    return '${date.year}-$month-$day';
  }

  /// 今日已用次数（记录的日期不是今天 ⇒ 0）。
  int usedToday([DateTime? now]) => date == todayKey(now) ? used : 0;

  /// 每日上限是否已到（`dailyCap = 0` ⇒ 不限，永远不到顶）。
  bool capReached([DateTime? now]) {
    final cap = dailyCap;
    return cap > 0 && usedToday(now) >= cap;
  }

  ProactiveConfig copyWith({
    bool? enabled,
    int? intervalMin,
    int? dailyCap,
    int? screenPollSec,
    int? screenMinGapSec,
    int? screenDiff,
  }) {
    return ProactiveConfig(
      enabled: enabled ?? this.enabled,
      intervalMin: intervalMin ?? this.intervalMin,
      dailyCap: dailyCap ?? this.dailyCap,
      screenPollSec: screenPollSec ?? this.screenPollSec,
      screenMinGapSec: screenMinGapSec ?? this.screenMinGapSec,
      screenDiff: screenDiff ?? this.screenDiff,
      date: date,
      used: used,
    );
  }
}

/// 桌宠窗口运行时状态。
class PetStatus {
  const PetStatus({this.open = false, this.error});

  final bool open;

  /// 最近一次失败原因（WebView2 / 引擎加载 / 模型解析）。
  final String? error;

  static PetStatus fromJson(Object? json) {
    if (json is! Map) {
      return const PetStatus();
    }
    final error = json['error'];
    return PetStatus(
      open: json['open'] == true,
      error: error is String && error.isNotEmpty ? error : null,
    );
  }
}

/// 一条桌宠事件（右键菜单 / 滚轮缩放 / 窗口关闭 / 渲染错误 / 引擎参数表 / 动作播放回执）。
class PetEvent {
  const PetEvent(
    this.type, {
    this.config,
    this.message,
    this.coreIds,
    this.ok = false,
  });

  /// `ready` / `console` / `closed` / `config` / `error` / `coreIds` / `motion`。
  final String type;
  final PetConfig? config;
  final String? message;

  /// `coreIds` 事件：引擎的运行时参数表（`parameters / drawables / parts /
  /// parameterRanges / physicsInputs / physicsOutputs`）。
  final Map<String, dynamic>? coreIds;

  /// `motion` 事件：动作播放是否成功。
  final bool ok;

  static PetEvent? fromJson(Object? json) {
    if (json is! Map) {
      return null;
    }
    final type = json['type'];
    if (type is! String || type.isEmpty) {
      return null;
    }
    final message = json['message'];
    final ids = json['ids'];
    return PetEvent(
      type,
      config: json['config'] == null
          ? null
          : PetConfig.fromJson(json['config']),
      message: message is String ? message : null,
      coreIds: ids is Map ? Map<String, dynamic>.from(ids) : null,
      ok: json['ok'] == true,
    );
  }
}

/// 模型校验结果（v1 `validateModelFile` 的 v2 版）。
class PetModelInfo {
  const PetModelInfo({
    this.ok = false,
    this.version = 0,
    this.textures = 0,
    this.motions = 0,
    this.expressions = 0,
    this.expressionNames = const [],
    this.motionGroups = const [],
    this.reason = '',
  });

  final bool ok;
  final double version;
  final int textures;
  final int motions;
  final int expressions;

  /// 模型声明的表情名（「模型调试」页直接列成按钮）。
  final List<String> expressionNames;

  /// 动作组名（`FileReferences.Motions` 的键）。
  final List<String> motionGroups;
  final String reason;

  static PetModelInfo fromJson(Object? json) {
    if (json is! Map) {
      return const PetModelInfo(reason: '模型校验失败');
    }
    int integer(String key) {
      final value = json[key];
      return value is num ? value.toInt() : 0;
    }

    List<String> names(String key) {
      final value = json[key];
      if (value is! List) {
        return const [];
      }
      return [
        for (final item in value)
          if (item is String && item.trim().isNotEmpty) item,
      ];
    }

    final version = json['version'];
    final reason = json['reason'];
    return PetModelInfo(
      ok: json['ok'] == true,
      version: version is num ? version.toDouble() : 0,
      textures: integer('textures'),
      motions: integer('motions'),
      expressions: integer('expressions'),
      expressionNames: names('expressionNames'),
      motionGroups: names('motionGroups'),
      reason: reason is String ? reason : '',
    );
  }
}

/// 一次桥接调用：库 / 符号缺失时返回 null（调用方回落到默认值）。
class _Bridge {
  const _Bridge(this.library);

  final DynamicLibrary library;

  String? callString(String symbol) {
    Pointer<Utf8> raw = nullptr;
    try {
      final call = library.lookupFunction<_StringC, _StringDart>(symbol);
      raw = call();
      if (raw == nullptr) {
        return null;
      }
      return raw.toDartString();
    } catch (_) {
      return null;
    } finally {
      _free(raw);
    }
  }

  String? callStringArg(String symbol, String argument) {
    Pointer<Utf8> raw = nullptr;
    final arg = argument.toNativeUtf8();
    try {
      final call = library.lookupFunction<_StrArgC, _StrArgDart>(symbol);
      raw = call(arg);
      if (raw == nullptr) {
        return null;
      }
      return raw.toDartString();
    } catch (_) {
      return null;
    } finally {
      _free(raw);
      malloc.free(arg);
    }
  }

  String? callBoolArg(String symbol, bool value) {
    Pointer<Utf8> raw = nullptr;
    try {
      final call = library.lookupFunction<_BoolArgC, _BoolArgDart>(symbol);
      raw = call(value);
      if (raw == nullptr) {
        return null;
      }
      return raw.toDartString();
    } catch (_) {
      return null;
    } finally {
      _free(raw);
    }
  }

  String? callUintArg(String symbol, int value) {
    Pointer<Utf8> raw = nullptr;
    try {
      final call = library.lookupFunction<_UintArgC, _UintArgDart>(symbol);
      raw = call(value);
      if (raw == nullptr) {
        return null;
      }
      return raw.toDartString();
    } catch (_) {
      return null;
    } finally {
      _free(raw);
    }
  }

  String? callTwoStringArgs(String symbol, String first, String second) {
    Pointer<Utf8> raw = nullptr;
    final a = first.toNativeUtf8();
    final b = second.toNativeUtf8();
    try {
      final call = library.lookupFunction<_TwoStrArgC, _TwoStrArgDart>(symbol);
      raw = call(a, b);
      if (raw == nullptr) {
        return null;
      }
      return raw.toDartString();
    } catch (_) {
      return null;
    } finally {
      _free(raw);
      malloc.free(a);
      malloc.free(b);
    }
  }

  String? callThreeStringArgs(
    String symbol,
    String first,
    String second,
    String third,
  ) {
    Pointer<Utf8> raw = nullptr;
    final a = first.toNativeUtf8();
    final b = second.toNativeUtf8();
    final c = third.toNativeUtf8();
    try {
      final call = library.lookupFunction<_ThreeStrArgC, _ThreeStrArgDart>(
        symbol,
      );
      raw = call(a, b, c);
      if (raw == nullptr) {
        return null;
      }
      return raw.toDartString();
    } catch (_) {
      return null;
    } finally {
      _free(raw);
      malloc.free(a);
      malloc.free(b);
      malloc.free(c);
    }
  }

  void _free(Pointer<Utf8> raw) {
    if (raw == nullptr) {
      return;
    }
    try {
      library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(raw);
    } catch (_) {
      // 旧库没有 free：宁可泄漏一次，也不要让面板崩掉。
    }
  }
}

_Bridge? _bridge() {
  final library = openLunacNative().library;
  return library == null ? null : _Bridge(library);
}

Map<String, Object?> _decode(String? text) {
  if (text == null || text.isEmpty) {
    return const {};
  }
  try {
    final value = jsonDecode(text);
    return value is Map ? Map<String, Object?>.from(value) : const {};
  } catch (_) {
    return const {};
  }
}

/// 读配置（桥接不可用时返回默认值）。
PetConfig readPetConfig() =>
    PetConfig.fromJson(_decode(_bridge()?.callString('lunac_pet_config')));

/// 合并保存配置（只传改动的字段）。返回保存后的完整配置。
PetConfig savePetConfig(Map<String, Object?> patch) {
  final bridge = _bridge();
  if (bridge == null) {
    return const PetConfig();
  }
  final saved = _decode(
    bridge.callStringArg('lunac_pet_apply', jsonEncode(patch)),
  );
  final error = saved['error'];
  if (error is String) {
    return readPetConfig();
  }
  return PetConfig.fromJson(saved);
}

/// 合并保存「主动反应」配置（v1 `ProactiveCfg`）。返回 `(config, error)`：
/// 出错时 config 是盘上现值，error 是一句人话（面板据此红字提示）。
({PetConfig config, String? error}) savePetProactive(
  Map<String, Object?> patch,
) {
  final bridge = _bridge();
  if (bridge == null) {
    return (config: const PetConfig(), error: '不可用（rust 桥接未加载）');
  }
  final saved = _decode(
    bridge.callStringArg('lunac_pet_set_proactive', jsonEncode(patch)),
  );
  final error = saved['error'];
  if (error is String && error.isNotEmpty) {
    return (config: readPetConfig(), error: error);
  }
  return (config: PetConfig.fromJson(saved), error: null);
}

/// 显示桌宠窗。
PetStatus showPetWindow() =>
    PetStatus.fromJson(_decode(_bridge()?.callString('lunac_pet_show')));

/// 隐藏桌宠窗（保留渲染器）。
PetStatus hidePetWindow() =>
    PetStatus.fromJson(_decode(_bridge()?.callString('lunac_pet_hide')));

/// 退出桌宠窗（销毁 WebView2）。
PetStatus quitPetWindow() =>
    PetStatus.fromJson(_decode(_bridge()?.callString('lunac_pet_quit')));

/// 鼠标穿透（v1 口径：只能在控制台关掉）。
PetStatus setPetClickThrough(bool on) => PetStatus.fromJson(
  _decode(_bridge()?.callBoolArg('lunac_pet_set_click_through', on)),
);

/// 置顶开关。
PetStatus setPetTopmost(bool on) => PetStatus.fromJson(
  _decode(_bridge()?.callBoolArg('lunac_pet_set_topmost', on)),
);

/// 重置窗口位置（回主屏右下）。
PetStatus resetPetPosition() => PetStatus.fromJson(
  _decode(_bridge()?.callString('lunac_pet_reset_position')),
);

/// 读运行时状态。
PetStatus readPetStatus() =>
    PetStatus.fromJson(_decode(_bridge()?.callString('lunac_pet_state')));

/// 取事件队列并清空（控制台 / 主页面轮询用）。
List<PetEvent> pollPetEvents() {
  final text = _bridge()?.callString('lunac_pet_poll');
  if (text == null || text.isEmpty) {
    return const [];
  }
  try {
    final decoded = jsonDecode(text);
    if (decoded is! List) {
      return const [];
    }
    final events = <PetEvent>[];
    for (final item in decoded) {
      final event = PetEvent.fromJson(item);
      if (event != null) {
        events.add(event);
      }
    }
    return events;
  } catch (_) {
    return const [];
  }
}

/// 系统的「选择形象文件」对话框；取消 / 桥接不可用返回 null。
String? pickPetShapeFile() {
  final path = _bridge()?.callString('lunac_pet_pick_file');
  if (path == null || path.trim().isEmpty) {
    return null;
  }
  return path.trim();
}

/// 校验 `*.model3.json`。
PetModelInfo? validatePetModel(String path) {
  final text = _bridge()?.callStringArg('lunac_pet_model_info', path);
  if (text == null) {
    return null;
  }
  return PetModelInfo.fromJson(_decode(text));
}

/// 「模型调试」一条指令的结果。
class PetDebugResult {
  const PetDebugResult({this.ok = false, this.open = false, this.error = ''});

  final bool ok;

  /// 桌宠窗当前是否打开（false = 值已记住，点「显示桌宠」后生效）。
  final bool open;
  final String error;

  static PetDebugResult fromJson(Object? json) {
    if (json is! Map) {
      return const PetDebugResult(error: '调试指令没有返回结果');
    }
    final error = json['error'];
    return PetDebugResult(
      ok: json['ok'] == true,
      open: json['open'] == true,
      error: error is String ? error : '',
    );
  }
}

/// 发一条「模型调试」指令（表情 / 参数）。
///
/// `persist:false` = 只预览不落盘（拖滑块时用；松手再发一次 `persist:true`）。
PetDebugResult sendPetDebug(Map<String, Object?> action) {
  final bridge = _bridge();
  if (bridge == null) {
    return const PetDebugResult(error: '不可用（rust 桥接未加载）');
  }
  return PetDebugResult.fromJson(
    _decode(bridge.callStringArg('lunac_pet_debug', jsonEncode(action))),
  );
}

// ── 桌宠人格 / 记忆（v1 `config/pet-persona.md` + `MemoryScope::Pet`）──
//
// v1 的六个控制台页签里，「人格」页的数据来源是四份文件：
//   · 核心人格 `config/pet-persona.md`（可改可清空）
//   · 事实记忆 `ModuleData/memory/pet/MEMORY.md`（AI 写，用户逐条删 / 清空）
//   · 成长笔记 `ModuleData/memory/pet/GROWTH.md`（同上）
//   · 人格建议 `ModuleData/memory/pet/PERSONA_SUGGESTIONS.md`（用户「接受」后并进人格）
// 本层只做读写与容错解析；文件格式与上限在 `rust/src/memory.rs`。

/// 桌宠人格（`lunac_pet_persona` 的返回）。
class PetPersona {
  const PetPersona({
    this.ok = false,
    this.path = '',
    this.text = '',
    this.maxChars = 8000,
    this.error = '',
  });

  final bool ok;
  final String path;
  final String text;
  final int maxChars;
  final String error;

  static PetPersona fromJson(Object? json) {
    if (json is! Map) {
      return const PetPersona(error: '人格读取失败');
    }
    final maxChars = json['maxChars'];
    return PetPersona(
      ok: json['ok'] == true,
      path: json['path'] is String ? json['path'] as String : '',
      text: json['text'] is String ? json['text'] as String : '',
      maxChars: maxChars is num ? maxChars.toInt() : 8000,
      error: json['error'] is String ? json['error'] as String : '',
    );
  }
}

// ══════════════════════════════════════════════════════════════════
//  旁挂文件 / 动作文件（v1 `read_pet_hotkeys` / `list_motion_files` 的对应物）
// ══════════════════════════════════════════════════════════════════
//
// 文件布局由 Rust 侧统一收口（`<模型目录>\lunac-live2d\`）；Dart 只负责调用与解析。
// 失败一律返回一句人话（error），不抛异常 —— 面板据此红字提示。

/// 旁挂文件（`hotkeys` / `chains`）的读取结果：文件不存在时 `text == null` 且无 error。
class PetSideFile {
  const PetSideFile({this.text, this.error});

  final String? text;
  final String? error;

  bool get ok => error == null;
}

/// 读旁挂文件（`kind`：`hotkeys` / `chains`）。
PetSideFile readPetSide(String modelPath, String kind) {
  final bridge = _bridge();
  if (bridge == null) {
    return const PetSideFile(error: '不可用（rust 桥接未加载）');
  }
  final value = _decode(
    bridge.callTwoStringArgs('lunac_pet_side_read', modelPath, kind),
  );
  if (value['ok'] != true) {
    return PetSideFile(
      error: value['error'] is String ? value['error'] as String : '读取失败',
    );
  }
  final text = value['text'];
  return PetSideFile(text: text is String ? text : null);
}

/// 写旁挂文件；成功返回落点路径。
({String? path, String? error}) writePetSide(
  String modelPath,
  String kind,
  String json,
) {
  final bridge = _bridge();
  if (bridge == null) {
    return (path: null, error: '不可用（rust 桥接未加载）');
  }
  final value = _decode(
    bridge.callThreeStringArgs('lunac_pet_side_write', modelPath, kind, json),
  );
  if (value['ok'] != true) {
    return (
      path: null,
      error: value['error'] is String ? value['error'] as String : '写入失败',
    );
  }
  final path = value['path'];
  return (path: path is String ? path : null, error: null);
}

/// 桌面宠语音状态（`config/pet-voice.json` + GPT-SoVITS / ZipVoice 体检）。
///
/// 默认引擎是 GPT-SoVITS：`serverUrl` 指向本机 gsv.cpp 的 `/tts` 服务；
/// `canSpeak` 只有在服务连通、参考音频文件存在、参考文本非空时才为 true。
class PetVoiceStatus {
  const PetVoiceStatus({
    this.enabled = true,
    this.engine = 'gpt_sovits',
    this.serverUrl = 'http://127.0.0.1:9880',
    this.sampleSteps = 4,
    this.textLang = 'all_zh',
    this.modelDir = '',
    this.modelFiles = 0,
    this.device = 'vulkan',
    this.autoStart = true,
    this.githubUrl = 'https://github.com/LythrumMoon/lunac-plugins/releases',
    this.hfUrl = 'https://huggingface.co/KakaruHayate/GSV.CPP',
    this.referenceAudio = '',
    this.referenceText = '',
    this.speed = 1.0,
    this.volume = 1.0,
    this.numSteps = 8,
    this.numThreads = 4,
    this.assetsReady = false,
    this.serverReady = false,
    this.assetDir = '',
    this.missingAssets = const [],
    this.speedMin = 0.5,
    this.speedMax = 2.0,
    this.volumeMin = 0.0,
    this.volumeMax = 1.0,
    this.reason = '',
    this.reasonText = '',
    this.canSpeak = false,
    this.path = '',
    this.error,
  });

  final bool enabled;
  final String engine;
  final String serverUrl;
  final int sampleSteps;
  final String textLang;
  final String modelDir;
  final int modelFiles;
  final String device;
  final bool autoStart;
  final String githubUrl;
  final String hfUrl;
  final String referenceAudio;
  final String referenceText;
  final double speed;
  final double volume;
  final int numSteps;
  final int numThreads;
  final bool assetsReady;
  final bool serverReady;
  final String assetDir;
  final List<String> missingAssets;
  final double speedMin;
  final double speedMax;
  final double volumeMin;
  final double volumeMax;

  /// 机器可读原因：`gsv_server_unreachable` / `no_reference` / `reference_missing` /
  /// `no_reference_text` / `ready` / 旧 ZipVoice 的 `assets_missing` 等。
  final String reason;

  /// 面板直接展示的一句话。
  final String reasonText;
  final bool canSpeak;

  /// `config/pet-voice.json` 的落点。
  final String path;

  /// 桥接不可用时的原因（正常时 null）。
  final String? error;

  bool get ok => error == null;

  static PetVoiceStatus? fromJson(Object? json) {
    if (json is! Map) {
      return null;
    }
    double number(Object? value, double fallback) =>
        value is num ? value.toDouble() : fallback;
    int integer(Object? value, int fallback) => value is int ? value : fallback;
    String text(Object? value) => value is String ? value : '';
    final missing = json['missingAssets'];
    return PetVoiceStatus(
      enabled: json['enabled'] != false,
      engine: text(json['engine']).isEmpty
          ? 'gpt_sovits'
          : text(json['engine']),
      serverUrl: text(json['serverUrl']).isEmpty
          ? 'http://127.0.0.1:9880'
          : text(json['serverUrl']),
      sampleSteps: integer(json['sampleSteps'], 4),
      textLang: text(json['textLang']).isEmpty
          ? 'all_zh'
          : text(json['textLang']),
      modelDir: text(json['modelDir']),
      modelFiles: integer(json['modelFiles'], 0),
      device: text(json['device']).isEmpty ? 'vulkan' : text(json['device']),
      autoStart: json['autoStart'] != false,
      githubUrl: text(json['githubUrl']).isEmpty
          ? 'https://github.com/LythrumMoon/lunac-plugins/releases'
          : text(json['githubUrl']),
      hfUrl: text(json['hfUrl']).isEmpty
          ? 'https://huggingface.co/KakaruHayate/GSV.CPP'
          : text(json['hfUrl']),
      referenceAudio: text(json['referenceAudio']),
      referenceText: text(json['referenceText']),
      speed: number(json['speed'], 1.0),
      volume: number(json['volume'], 1.0),
      numSteps: integer(json['numSteps'], 8),
      numThreads: integer(json['numThreads'], 4),
      assetsReady: json['assetsReady'] == true,
      serverReady: json['serverReady'] == true,
      assetDir: text(json['assetDir']),
      missingAssets: [
        for (final item in (missing is List ? missing : const []))
          if (item is String && item.isNotEmpty) item,
      ],
      speedMin: number(json['speedMin'], 0.5),
      speedMax: number(json['speedMax'], 2.0),
      volumeMin: number(json['volumeMin'], 0.0),
      volumeMax: number(json['volumeMax'], 1.0),
      reason: text(json['reason']),
      reasonText: text(json['reasonText']),
      canSpeak: json['canSpeak'] == true,
      path: text(json['path']),
    );
  }
}

/// 读桌宠语音状态；桥接不可用时返回带 `error` 的对象（面板显示「不可用」）。
PetVoiceStatus readPetVoice() {
  final bridge = _bridge();
  if (bridge == null) {
    return const PetVoiceStatus(error: '不可用（rust 桥接未加载）');
  }
  final value = _decode(bridge.callString('lunac_pet_voice_status'));
  final status = PetVoiceStatus.fromJson(value);
  if (status == null) {
    return const PetVoiceStatus(error: '语音状态格式不对');
  }
  return status;
}

/// 合并保存语音配置（只传关心的字段），返回保存后的状态。
({bool ok, String? error, PetVoiceStatus? status}) savePetVoice(
  Map<String, Object?> patch,
) {
  final bridge = _bridge();
  if (bridge == null) {
    return (ok: false, error: '不可用（rust 桥接未加载）', status: null);
  }
  final value = _decode(
    bridge.callStringArg('lunac_pet_voice_set', jsonEncode(patch)),
  );
  final ok = value['ok'] == true;
  final error = value['error'];
  return (
    ok: ok,
    error: ok
        ? null
        : (error is String && error.isNotEmpty ? error : '保存语音配置失败'),
    status: PetVoiceStatus.fromJson(value['status']),
  );
}

/// 朗读一段文本（GPT-SoVITS / gsv.cpp）；返回 `(ok, error)`，音频在后台播放。
({bool ok, String? error}) speakPetVoice(String text) {
  final bridge = _bridge();
  if (bridge == null) {
    return (ok: false, error: '不可用（rust 桥接未加载）');
  }
  final value = _decode(bridge.callStringArg('lunac_pet_voice_speak', text));
  final ok = value['ok'] == true;
  final error = value['error'];
  return (
    ok: ok,
    error: ok ? null : (error is String && error.isNotEmpty ? error : '朗读失败'),
  );
}

/// 启动 GPT-SoVITS 服务（有 Start.bat 优先用它）；已有服务直接复用。
({bool ok, bool already, String? error}) startPetVoiceService() {
  final bridge = _bridge();
  if (bridge == null) {
    return (ok: false, already: false, error: '不可用（rust 桥接未加载）');
  }
  final value = _decode(bridge.callString('lunac_pet_voice_start'));
  final ok = value['ok'] == true;
  final error = value['error'];
  return (
    ok: ok,
    already: value['already'] == true,
    error: ok ? null : (error is String && error.isNotEmpty ? error : '启动服务失败'),
  );
}

/// 停掉由本进程启动的服务（换模型目录时用）。
void stopPetVoiceService() {
  final bridge = _bridge();
  if (bridge == null) {
    return;
  }
  _decode(bridge.callString('lunac_pet_voice_stop'));
}

/// 选择参考音频（单选）；取消 / 桥接缺失返回 null。
String? pickPetVoiceAudio() {
  final bridge = _bridge();
  if (bridge == null) {
    return null;
  }
  final value = _decode(bridge.callString('lunac_pet_voice_pick_audio'));
  if (value['canceled'] == true || value['ok'] != true) {
    return null;
  }
  final path = value['path'];
  return path is String && path.isNotEmpty ? path : null;
}

/// 一条桌宠热键（`lunac-live2d/lunac.hotkeys.json` 的 `hotkeys[]`）。
///
/// `action` 原样保留（`type` 判别联合：motion / chain / expression / param / artmesh /
/// artmeshColor / part / physics）—— 面板只展示与增删，**不改写**动作细节，
/// 这样 AI / 用户手改过的字段不会被落盘时抹掉。
class PetHotkey {
  const PetHotkey({
    required this.id,
    required this.name,
    required this.keys,
    required this.action,
  });

  final String id;
  final String name;

  /// 规范化按键串（如 `Ctrl+Numpad1`）；v2 里只作 AI 侧标识。
  final String keys;
  final Map<String, Object?> action;

  String get actionType =>
      action['type'] is String ? action['type'] as String : '';

  /// 动作的一句话描述（面板列表用）。
  String get actionLabel {
    String text(Object? value) => value is String ? value : '';
    final target = text(action['target']);
    final id = text(action['id']);
    final value = action['value'];
    switch (actionType) {
      case 'motion':
        final group = text(action['group']);
        final index = action['index'];
        return '动作 ${group.isEmpty ? '（未选组）' : group} #$index';
      case 'chain':
        return '动作链 $id';
      case 'expression':
        final mode = action['mode'] == 'clear' ? '关闭' : '开启';
        return '表情 $target（$mode）';
      case 'param':
        final mode = action['mode'] == 'add' ? '+=' : '=';
        return '参数 $id $mode $value';
      case 'artmesh':
        return 'ArtMesh $id = $value';
      case 'artmeshColor':
        return '染色 $id ${text(action['color'])}';
      case 'part':
        return 'Part $id = $value';
      case 'physics':
        return action['enabled'] == true ? '物理：开' : '物理：关';
      default:
        return actionType.isEmpty ? '（无动作）' : actionType;
    }
  }

  Map<String, Object?> toJson() => {
    'id': id,
    'name': name,
    'keys': keys,
    'action': action,
  };

  static List<PetHotkey> listFromJson(Object? value) {
    if (value is! List) {
      return const [];
    }
    final out = <PetHotkey>[];
    for (final item in value) {
      if (item is! Map) {
        continue;
      }
      final keys = item['keys'];
      if (keys is! String) {
        // v1 同口径：没有 keys 的条目视为坏数据（否则热键表会被半截条目撑爆）。
        continue;
      }
      out.add(
        PetHotkey(
          id: item['id'] is String ? item['id'] as String : '',
          name: item['name'] is String ? item['name'] as String : '',
          keys: keys,
          action: item['action'] is Map
              ? {
                  for (final entry in (item['action'] as Map).entries)
                    '${entry.key}': entry.value,
                }
              : const {},
        ),
      );
    }
    return out;
  }
}

/// 读模型的热键表（文件不存在 / 内容坏掉 ⇒ 空表，与 v1 一致）。
List<PetHotkey> readPetHotkeys(String modelPath) {
  final side = readPetSide(modelPath, 'hotkeys');
  final text = side.text;
  if (text == null || text.trim().isEmpty) {
    return const [];
  }
  try {
    final decoded = jsonDecode(text);
    if (decoded is! Map) {
      return const [];
    }
    return PetHotkey.listFromJson(decoded['hotkeys']);
  } catch (_) {
    return const [];
  }
}

/// 写回热键表（`{version:1, model:<模型文件名>, hotkeys:[…]}`）；成功返回落点路径。
({String? path, String? error}) writePetHotkeys(
  String modelPath,
  List<PetHotkey> hotkeys,
) {
  final model = modelPath.split(RegExp(r'[\\/]')).last;
  final payload = const JsonEncoder.withIndent('  ').convert({
    'version': 1,
    'model': model,
    'hotkeys': [for (final hotkey in hotkeys) hotkey.toJson()],
  });
  return writePetSide(modelPath, 'hotkeys', payload);
}

/// 导入时给条目自动分配的默认按键串（v1 `defaultKeysForIndex` 同口径）。
///
/// 基键取小键盘 1..9，逐层叠修饰键（7 层 × 9 = 63 个互不重复），超出退回裸数字。
/// v2 里按键只作 AI 侧标识，本地不做按键监听。
String defaultPetHotkeyKeys(int index) {
  const bases = [
    'Numpad1',
    'Numpad2',
    'Numpad3',
    'Numpad4',
    'Numpad5',
    'Numpad6',
    'Numpad7',
    'Numpad8',
    'Numpad9',
  ];
  const tiers = [
    'Ctrl',
    'Shift',
    'Alt',
    'Ctrl+Shift',
    'Ctrl+Alt',
    'Shift+Alt',
    'Ctrl+Shift+Alt',
  ];
  final safe = index < 0 ? 0 : index;
  final base = bases[safe % bases.length];
  final tier = safe ~/ bases.length;
  return tier < tiers.length ? '${tiers[tier]}+$base' : base;
}

/// 一个动作 / 表情文件（`{ 路径, 展示名 }`）。
class PetMotionFile {
  const PetMotionFile({required this.path, required this.name});

  final String path;
  final String name;

  /// 磁盘上的文件名（`xxx.motion3.json`）—— 改名 / 删除接口按它收口。
  String get fileName => path.split(RegExp(r'[\\/]')).last;

  static List<PetMotionFile> listFromJson(Object? value) {
    if (value is! List) {
      return const [];
    }
    return [
      for (final item in value)
        if (item is Map && item['path'] is String)
          PetMotionFile(
            path: item['path'] as String,
            name: item['name'] is String && (item['name'] as String).isNotEmpty
                ? item['name'] as String
                : _stem(item['path'] as String),
          ),
    ];
  }

  static String _stem(String path) {
    final name = path.split(RegExp(r'[\\/]')).last;
    return name.endsWith('.motion3.json')
        ? name.substring(0, name.length - '.motion3.json'.length)
        : name;
  }
}

/// 列模型目录下全部 `*.motion3.json`（顺序即引擎组内序号）。
({List<PetMotionFile> files, String? error}) readPetMotionFiles(
  String modelPath,
) {
  final bridge = _bridge();
  if (bridge == null) {
    return (files: const [], error: '不可用（rust 桥接未加载）');
  }
  final value = _decode(
    bridge.callStringArg('lunac_pet_motion_files', modelPath),
  );
  if (value['ok'] != true) {
    return (
      files: const [],
      error: value['error'] is String ? value['error'] as String : '读取失败',
    );
  }
  return (files: PetMotionFile.listFromJson(value['files']), error: null);
}

/// 新建一个动作文件（已存在则拒绝）。
({String? path, String? error}) writePetMotionFile(
  String modelPath,
  String name,
  String contents,
) {
  final bridge = _bridge();
  if (bridge == null) {
    return (path: null, error: '不可用（rust 桥接未加载）');
  }
  final value = _decode(
    bridge.callThreeStringArgs(
      'lunac_pet_motion_write',
      modelPath,
      name,
      contents,
    ),
  );
  if (value['ok'] != true) {
    return (
      path: null,
      error: value['error'] is String ? value['error'] as String : '新建失败',
    );
  }
  final path = value['path'];
  return (path: path is String ? path : null, error: null);
}

/// 删除一个动作文件。
({bool ok, String? error}) deletePetMotionFile(String modelPath, String name) {
  final bridge = _bridge();
  if (bridge == null) {
    return (ok: false, error: '不可用（rust 桥接未加载）');
  }
  final value = _decode(
    bridge.callTwoStringArgs('lunac_pet_motion_delete', modelPath, name),
  );
  return (
    ok: value['ok'] == true,
    error: value['error'] is String ? value['error'] as String : null,
  );
}

/// 重命名一个动作文件（新名留在原目录）。
({String? path, String? error}) renamePetMotionFile(
  String modelPath,
  String from,
  String to,
) {
  final bridge = _bridge();
  if (bridge == null) {
    return (path: null, error: '不可用（rust 桥接未加载）');
  }
  final value = _decode(
    bridge.callThreeStringArgs('lunac_pet_motion_rename', modelPath, from, to),
  );
  if (value['ok'] != true) {
    return (
      path: null,
      error: value['error'] is String ? value['error'] as String : '重命名失败',
    );
  }
  final path = value['path'];
  return (path: path is String ? path : null, error: null);
}

/// 递归列出模型目录下的 `*.exp3.json`（热键导入的回落数据源）。
({List<PetMotionFile> files, String? error}) readPetExp3Files(
  String modelPath,
) {
  final bridge = _bridge();
  if (bridge == null) {
    return (files: const [], error: '不可用（rust 桥接未加载）');
  }
  final value = _decode(
    bridge.callStringArg('lunac_pet_exp3_files', modelPath),
  );
  if (value['ok'] != true) {
    return (
      files: const [],
      error: value['error'] is String ? value['error'] as String : '读取失败',
    );
  }
  return (files: PetMotionFile.listFromJson(value['files']), error: null);
}

/// 模型文件夹根上的三类关键文件（`*.model3.json` / `*.vtube.json` / `*.cdi3.json`）。
class PetFolderScan {
  const PetFolderScan({
    this.model3 = const [],
    this.vtube = const [],
    this.cdi3 = const [],
  });

  final List<String> model3;
  final List<String> vtube;
  final List<String> cdi3;

  static List<String> _paths(Object? value) {
    if (value is! List) {
      return const [];
    }
    return [
      for (final item in value)
        if (item is String && item.isNotEmpty) item,
    ];
  }

  static PetFolderScan fromJson(Object? json) {
    if (json is! Map) {
      return const PetFolderScan();
    }
    return PetFolderScan(
      model3: _paths(json['model3']),
      vtube: _paths(json['vtube']),
      cdi3: _paths(json['cdi3']),
    );
  }
}

/// 扫模型文件夹（不递归）。
({PetFolderScan scan, String? error}) scanPetFolder(String modelPath) {
  final bridge = _bridge();
  if (bridge == null) {
    return (scan: const PetFolderScan(), error: '不可用（rust 桥接未加载）');
  }
  final value = _decode(
    bridge.callStringArg('lunac_pet_scan_folder', modelPath),
  );
  if (value['ok'] != true) {
    return (
      scan: const PetFolderScan(),
      error: value['error'] is String ? value['error'] as String : '扫描失败',
    );
  }
  return (scan: PetFolderScan.fromJson(value['scan']), error: null);
}

/// 请求桌宠窗引擎上报运行时参数表；结果从 [pollPetEvents] 的 `coreIds` 事件取。
({bool ok, String? error}) requestPetCoreIds() {
  final bridge = _bridge();
  if (bridge == null) {
    return (ok: false, error: '不可用（rust 桥接未加载）');
  }
  final value = _decode(bridge.callString('lunac_pet_request_core_ids'));
  return (
    ok: value['ok'] == true,
    error: value['error'] is String && (value['error'] as String).isNotEmpty
        ? value['error'] as String
        : null,
  );
}

/// 播放 `Lunac` 合成组里的第 [index] 个动作（行号 = 动作清单顺序）。
({bool ok, String? error}) playPetMotion(int index) {
  final bridge = _bridge();
  if (bridge == null) {
    return (ok: false, error: '不可用（rust 桥接未加载）');
  }
  final value = _decode(bridge.callUintArg('lunac_pet_play_motion', index));
  return (
    ok: value['ok'] == true,
    error: value['error'] is String && (value['error'] as String).isNotEmpty
        ? value['error'] as String
        : null,
  );
}

/// 模型的物理输出参数 id（新建动作模板默认排除它们）。
({List<String> ids, String? error}) readPetPhysicsOutputs(String modelPath) {
  final bridge = _bridge();
  if (bridge == null) {
    return (ids: const [], error: '不可用（rust 桥接未加载）');
  }
  final value = _decode(
    bridge.callStringArg('lunac_pet_physics_outputs', modelPath),
  );
  if (value['ok'] != true) {
    return (
      ids: const [],
      error: value['error'] is String ? value['error'] as String : '读取失败',
    );
  }
  final raw = value['ids'];
  return (
    ids: [
      for (final id in (raw is List ? raw : const []))
        if (id is String && id.isNotEmpty) id,
    ],
    error: null,
  );
}

/// 按运行时参数表生成一个 motion3 模板（v1 `buildMotionTemplate` 同口径）。
///
/// 每个参数一条**常数曲线**（取模型默认值），时长 1s / 30fps / 循环 —— 这不是「好看的动作」，
/// 只是把参数铺齐，留给 AI 往里编曲线。`includePhysics=false` 时排除物理输出参数
/// （它们每帧被物理写值，铺了也会被覆盖）。参数表为空 ⇒ null（调用方提示先显示桌宠）。
String? buildPetMotionTemplate({
  required Map<String, dynamic> coreIds,
  required Set<String> physicsOutputs,
  required bool includePhysics,
}) {
  final rawParams = coreIds['parameters'];
  final parameters = [
    for (final id in (rawParams is List ? rawParams : const []))
      if (id is String && id.isNotEmpty) id,
  ];
  if (parameters.isEmpty) {
    return null;
  }
  final ranges = coreIds['parameterRanges'];
  final rangeMap = ranges is Map ? ranges : const {};
  final curves = <Map<String, Object?>>[];
  for (final id in parameters) {
    if (!includePhysics && physicsOutputs.contains(id)) {
      continue;
    }
    final range = rangeMap[id];
    final def = range is Map ? range['def'] : null;
    final value = def is num && def.isFinite ? def.toDouble() : 0.0;
    curves.add({
      'Target': 'Parameter',
      'Id': id,
      'Segments': [0, value, 0, 1, value],
    });
  }
  if (curves.isEmpty) {
    return null;
  }
  return const JsonEncoder.withIndent('  ').convert({
    'Version': 3,
    'Meta': {
      'Duration': 1,
      'Fps': 30,
      'Loop': true,
      'AreBeziersRestricted': true,
      'CurveCount': curves.length,
      'TotalSegmentCount': curves.length,
      'TotalPointCount': curves.length * 2,
      'UserDataCount': 0,
      'TotalUserDataSize': 0,
    },
    'Curves': curves,
    'UserData': <Object?>[],
  });
}

/// 桌宠三份列表（记忆 / 成长笔记 / 人格建议）。
enum PetMemoryKind { memory, growth, suggestions }

/// 一份列表 + 两条上限（上限由 Rust 常量回显）。
class PetMemoryList {
  const PetMemoryList({
    this.entries = const [],
    this.maxEntryChars = 2000,
    this.maxTotalChars = 6000,
    this.error = '',
  });

  final List<String> entries;
  final int maxEntryChars;
  final int maxTotalChars;
  final String error;

  static PetMemoryList fromJson(Object? json) {
    if (json is! Map) {
      return const PetMemoryList(error: '读取失败');
    }
    final rawEntries = json['entries'];
    return PetMemoryList(
      entries: [
        for (final item in (rawEntries is List ? rawEntries : const []))
          if (item is String) item,
      ],
      maxEntryChars: json['maxEntryChars'] is num
          ? (json['maxEntryChars'] as num).toInt()
          : 2000,
      maxTotalChars: json['maxTotalChars'] is num
          ? (json['maxTotalChars'] as num).toInt()
          : 6000,
      error: json['error'] is String ? json['error'] as String : '',
    );
  }
}

/// 读桌宠人格（桥接缺失 = 默认值 + 原因）。
PetPersona readPetPersona() =>
    PetPersona.fromJson(_decode(_bridge()?.callString('lunac_pet_persona')));

/// 保存桌宠人格。返回 null = 成功，否则是一句原因。
String? savePetPersona(String text) {
  final bridge = _bridge();
  if (bridge == null) {
    return '不可用（rust 桥接未加载）';
  }
  final result = _decode(bridge.callStringArg('lunac_pet_set_persona', text));
  if (result.isEmpty) {
    return '保存失败（rust 桥接未返回结果）';
  }
  if (result['ok'] == true) {
    return null;
  }
  final error = result['error'];
  return error is String && error.isNotEmpty ? error : '保存人格失败';
}

/// 读一份桌宠列表；桥接缺失返回 null。
PetMemoryList? readPetMemory(PetMemoryKind kind) {
  final text = _bridge()?.callStringArg('lunac_pet_memory', kind.name);
  if (text == null || text.isEmpty) {
    return null;
  }
  return PetMemoryList.fromJson(_decode(text));
}

/// 删一条（按内容比对）。返回 null = 成功，否则是一句原因。
String? deletePetMemoryEntry(PetMemoryKind kind, String entry) {
  final bridge = _bridge();
  if (bridge == null) {
    return '不可用（rust 桥接未加载）';
  }
  final result = _decode(
    bridge.callTwoStringArgs('lunac_pet_memory_delete', kind.name, entry),
  );
  return _resultError(result, '删除失败');
}

/// 清空一份列表。返回 null = 成功，否则是一句原因。
String? clearPetMemory(PetMemoryKind kind) {
  final bridge = _bridge();
  if (bridge == null) {
    return '不可用（rust 桥接未加载）';
  }
  final result = _decode(
    bridge.callStringArg('lunac_pet_memory_clear', kind.name),
  );
  return _resultError(result, '清空失败');
}

/// 「接受」一条人格建议（并进人格后从建议区摘掉）。返回 null = 成功。
String? mergePetPersonaSuggestion(String entry) {
  final bridge = _bridge();
  if (bridge == null) {
    return '不可用（rust 桥接未加载）';
  }
  final result = _decode(
    bridge.callStringArg('lunac_pet_merge_suggestion', entry),
  );
  return _resultError(result, '合并失败');
}

String? _resultError(Map<String, Object?> result, String fallback) {
  if (result.isEmpty) {
    return '$fallback（rust 桥接未返回结果）';
  }
  if (result['ok'] == true) {
    return null;
  }
  final error = result['error'];
  return error is String && error.isNotEmpty ? error : fallback;
}

/// 当前是否 Live2D 模型文件（控制台按扩展名分流，与 v1 的导入对话框一致）。
bool isLive2DModelPath(String path) =>
    path.toLowerCase().endsWith('.model3.json');

/// 当前是否是支持的图片（png / jpg / jpeg / webp / gif）。
bool isPetImagePath(String path) {
  final lower = path.toLowerCase();
  return lower.endsWith('.png') ||
      lower.endsWith('.jpg') ||
      lower.endsWith('.jpeg') ||
      lower.endsWith('.webp') ||
      lower.endsWith('.gif');
}
