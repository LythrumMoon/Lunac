// 平台薄层：外观（系统强调色 / 深浅色 + 主题包清单 + 主题目录 + 当前主题）。
//
// 边界：主题包的扫描、JSON 解析、资产路径校验（含 `..` / 绝对路径防护）都在
// `rust/src/appearance.rs`；本文件只把 C ABI 包成 Dart 结构，并把主题 tokens
// 应用到运行时色板（`LunacColors.apply`）。
//
// 2026-11-14 起主题包可以真正切换：设置「风格」里点主题泡泡 → `setAppearanceTheme`
// 落 `config/appearance.json` → 当前引擎立刻重建；三处 Flutter 入口在 runApp 之前调
// `applySavedTheme`，所以重开窗口 / 重启应用都会保持选中的主题。卫星窗口（插件窗 /
// 附件抽屉）是独立 engine，靠 [startAppearanceWatch] 低频轮询外观配置跟上主窗，
// 等价 v1 里「同一个 WebView 内换主题所有面板立刻变」。
import 'dart:async';
import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';
import 'package:flutter/material.dart' show Color, HSLColor, Widget;

import '../theme.dart';
import 'native_bridge.dart';

/// 系统强调色与深 / 浅色偏好（与 Rust `SystemTheme` 一一对应）。
class SystemTheme {
  const SystemTheme({
    required this.accent,
    required this.dark,
    required this.source,
  });

  /// "#RRGGBB"；读不到时为空串。
  final String accent;

  /// 系统是否为深色。
  final bool dark;

  /// `registry` = 真读到了注册表；`fallback` = 用的默认值。
  final String source;

  static SystemTheme? fromJson(Map<String, dynamic> json) {
    final dark = json['dark'];
    final source = json['source'];
    if (dark is! bool || source is! String) {
      return null;
    }
    return SystemTheme(
      accent: json['accent'] is String ? json['accent'] as String : '',
      dark: dark,
      source: source,
    );
  }
}

/// 一个已安装的主题包（只取设置面板要展示的字段）。
class ThemePackage {
  const ThemePackage({
    required this.id,
    required this.name,
    required this.version,
    required this.author,
    required this.builtin,
    required this.dir,
    required this.accent,
    required this.hasBackground,
    required this.iconCount,
    this.tokens = const {},
    this.background,
  });

  final String id;
  final String name;
  final String version;
  final String author;

  /// 目录内有 `builtin` 标记文件 = 随应用维护的内置主题。
  final bool builtin;

  /// 主题目录绝对路径。
  final String dir;

  /// `tokens.accent`（可能为空）。
  final String accent;

  /// 是否声明了背景图（Rust 已解析成绝对路径并确认存在）。
  final bool hasBackground;

  /// 已解析出绝对路径的插件图标数。
  final int iconCount;

  /// 主题色板原文（`manifest.tokens` 里所有字符串字段；数字字段不进这里）。
  final Map<String, String> tokens;

  /// 主题背景图绝对路径（Rust 已确认存在）；null = 该主题没声明背景。
  final String? background;

  static ThemePackage? fromJson(Map<String, dynamic> json) {
    final manifest = json['manifest'];
    if (manifest is! Map<String, dynamic>) {
      return null;
    }
    final id = manifest['id'];
    if (id is! String) {
      return null;
    }
    final tokens = manifest['tokens'];
    final resolved = json['resolved'];
    final icons = resolved is Map ? resolved['icons'] : null;
    return ThemePackage(
      id: id,
      name: manifest['name'] is String ? manifest['name'] as String : id,
      version: manifest['version'] is String
          ? manifest['version'] as String
          : '',
      author: manifest['author'] is String ? manifest['author'] as String : '',
      builtin: json['builtin'] == true,
      dir: json['dir'] is String ? json['dir'] as String : '',
      accent: tokens is Map && tokens['accent'] is String
          ? tokens['accent'] as String
          : '',
      hasBackground: resolved is Map && resolved['background'] is String,
      iconCount: icons is Map ? icons.length : 0,
      tokens: {
        if (tokens is Map)
          for (final entry in tokens.entries)
            if (entry.value is String) '${entry.key}': entry.value as String,
      },
      background: resolved is Map && resolved['background'] is String
          ? resolved['background'] as String
          : null,
    );
  }
}

typedef _GetC = Pointer<Utf8> Function();
typedef _GetDart = Pointer<Utf8> Function();
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

/// 调用一个「无参 → 字符串」的 C ABI；桥接缺失 / 符号缺失返回 null。
String? _callString(String symbol) {
  final library = openLunacNative().library;
  if (library == null) {
    return null;
  }
  Pointer<Utf8>? raw;
  try {
    raw = library.lookupFunction<_GetC, _GetDart>(symbol)();
    if (raw == nullptr) {
      return null;
    }
    return raw.toDartString();
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

/// 读系统主题；桥接缺失返回 null（界面显示「不可用」）。
SystemTheme? readSystemTheme() {
  final text = _callString('lunac_system_theme');
  if (text == null) {
    return null;
  }
  try {
    final value = jsonDecode(text);
    if (value is! Map<String, dynamic>) {
      return null;
    }
    return SystemTheme.fromJson(value);
  } catch (_) {
    return null;
  }
}

/// 读已安装主题包；桥接缺失返回 null（与「确实一个都没有 = 空表」区分开）。
List<ThemePackage>? readThemes() {
  final text = _callString('lunac_themes');
  if (text == null) {
    return null;
  }
  try {
    final value = jsonDecode(text);
    if (value is! List) {
      return null;
    }
    return [
      for (final item in value)
        if (item is Map<String, dynamic>) ?ThemePackage.fromJson(item),
    ];
  } catch (_) {
    return null;
  }
}

/// 主题目录绝对路径（Rust 侧不存在则创建）；桥接缺失返回 null。
String? themesDirectory() => _callString('lunac_themes_dir');

// ── 外观配置（`config/appearance.json`）────────────────────────────

/// 自定义配色（v1 localStorage `lunac-appearance` 的字段名与默认值）。
///
/// 颜色字段的**空串 = 跟随主题**（不是「黑色」）—— 没动过取色器时不能把主题色写死；
/// 数值字段越界由后端夹取（本类只保留原值做展示）。
class AppearanceConfig {
  const AppearanceConfig({
    this.themeId = 'default',
    this.baseColor = '',
    this.surfaceAlpha = 1,
    this.btnLineColor = '',
    this.btnLineAlpha = 1,
    this.btnBgColor = '',
    this.btnBgAlpha = 0.09,
    this.textLight = 0,
    this.overlayColor = '',
    this.overlayAlpha = 0.1,
    this.bgImage,
    this.bgBlur = 4,
    this.bgSaturate = 92,
    this.bgOpacity = 0.5,
    this.sheen = 0,
    this.tintBase = false,
  });

  final String themeId;
  final String baseColor;
  final double surfaceAlpha;
  final String btnLineColor;
  final double btnLineAlpha;
  final String btnBgColor;
  final double btnBgAlpha;
  final double textLight;
  final String overlayColor;
  final double overlayAlpha;
  final String? bgImage;
  final double bgBlur;
  final double bgSaturate;
  final double bgOpacity;
  final double sheen;

  /// v1 `tintBase`：「恢复默认主题」开关（配色回到主题自带的值）。
  final bool tintBase;

  static const AppearanceConfig defaults = AppearanceConfig();

  AppearanceConfig copyWith({
    String? themeId,
    String? baseColor,
    double? surfaceAlpha,
    String? btnLineColor,
    double? btnLineAlpha,
    String? btnBgColor,
    double? btnBgAlpha,
    double? textLight,
    String? overlayColor,
    double? overlayAlpha,
    String? bgImage,

    /// 显式清除背景图（`bgImage: null` 在 copyWith 里表示「保持原值」）。
    bool clearBgImage = false,
    double? bgBlur,
    double? bgSaturate,
    double? bgOpacity,
    double? sheen,
    bool? tintBase,
  }) {
    return AppearanceConfig(
      themeId: themeId ?? this.themeId,
      baseColor: baseColor ?? this.baseColor,
      surfaceAlpha: surfaceAlpha ?? this.surfaceAlpha,
      btnLineColor: btnLineColor ?? this.btnLineColor,
      btnLineAlpha: btnLineAlpha ?? this.btnLineAlpha,
      btnBgColor: btnBgColor ?? this.btnBgColor,
      btnBgAlpha: btnBgAlpha ?? this.btnBgAlpha,
      textLight: textLight ?? this.textLight,
      overlayColor: overlayColor ?? this.overlayColor,
      overlayAlpha: overlayAlpha ?? this.overlayAlpha,
      bgImage: clearBgImage ? null : (bgImage ?? this.bgImage),
      bgBlur: bgBlur ?? this.bgBlur,
      bgSaturate: bgSaturate ?? this.bgSaturate,
      bgOpacity: bgOpacity ?? this.bgOpacity,
      sheen: sheen ?? this.sheen,
      tintBase: tintBase ?? this.tintBase,
    );
  }

  static AppearanceConfig fromJson(Map<String, dynamic> json) {
    String text(String key, String fallback) =>
        json[key] is String ? json[key] as String : fallback;
    double number(String key, double fallback) =>
        json[key] is num ? (json[key] as num).toDouble() : fallback;
    return AppearanceConfig(
      themeId: text('themeId', 'default'),
      baseColor: text('baseColor', ''),
      surfaceAlpha: number('surfaceAlpha', 1),
      btnLineColor: text('btnLineColor', ''),
      btnLineAlpha: number('btnLineAlpha', 1),
      btnBgColor: text('btnBgColor', ''),
      btnBgAlpha: number('btnBgAlpha', 0.09),
      textLight: number('textLight', 0),
      overlayColor: text('overlayColor', ''),
      overlayAlpha: number('overlayAlpha', 0.1),
      bgImage: json['bgImage'] is String ? json['bgImage'] as String : null,
      bgBlur: number('bgBlur', 4),
      bgSaturate: number('bgSaturate', 92),
      bgOpacity: number('bgOpacity', 0.5),
      sheen: number('sheen', 0),
      tintBase: json['tintBase'] == true,
    );
  }
}

/// 弹出系统文件对话框选背景图；取消 / 桥接缺失返回 null。
String? pickBackgroundImage() {
  final path = _callString('lunac_appearance_pick_bg');
  return path == null || path.isEmpty ? null : path;
}

/// 面板内背景（搜索结果区 / AI 对话）：按当前外观配置返回一层只画图片的组件。
///
/// 只铺在调用方自己的裁剪区内，位置 / 圆角严格跟随面板；没有背景图 =
/// [SizedBox.shrink]。相同配置复用同一个 widget 实例，避免每次按键重建图片滤镜。
Widget appearancePanelBackdrop() {
  final config = readAppearanceConfig();
  final key = [
    config.bgImage ?? '',
    config.bgBlur.toStringAsFixed(2),
    config.bgSaturate.toStringAsFixed(2),
    config.bgOpacity.toStringAsFixed(3),
    config.sheen.toStringAsFixed(3),
  ].join('|');
  if (key == _panelBackdropKey && _panelBackdrop != null) {
    return _panelBackdrop!;
  }
  _panelBackdropKey = key;
  _panelBackdrop = AppearanceBackdropImage(
    imagePath: config.bgImage,
    blur: config.bgBlur,
    saturate: config.bgSaturate,
    opacity: config.bgOpacity,
    sheen: config.sheen,
  );
  return _panelBackdrop!;
}

String? _panelBackdropKey;
Widget? _panelBackdrop;

/// 读外观配置；桥接缺失 / 读坏 = 默认值。
AppearanceConfig readAppearanceConfig() {
  final raw = _callString('lunac_appearance_config');
  if (raw == null || raw.isEmpty) {
    return AppearanceConfig.defaults;
  }
  try {
    final value = jsonDecode(raw);
    if (value is! Map<String, dynamic>) {
      return AppearanceConfig.defaults;
    }
    return AppearanceConfig.fromJson(value);
  } catch (_) {
    return AppearanceConfig.defaults;
  }
}

/// 合并写外观配置（只传要改的键）。返回 null = 成功，否则是一句原因。
String? saveAppearanceConfig(Map<String, Object?> patch) {
  final library = openLunacNative().library;
  if (library == null) {
    return '当前环境不支持保存外观设置';
  }
  Pointer<Utf8>? input;
  Pointer<Utf8>? raw;
  try {
    final call = library.lookupFunction<_SetC, _SetDart>(
      'lunac_set_appearance_config',
    );
    input = jsonEncode(patch).toNativeUtf8();
    raw = call(input);
    if (raw == nullptr) {
      return '保存外观设置失败';
    }
    final value = jsonDecode(raw.toDartString());
    if (value is! Map<String, dynamic>) {
      return '保存外观设置失败';
    }
    if (value['ok'] == true) {
      return null;
    }
    final reason = value['error'];
    return reason is String && reason.isNotEmpty ? reason : '保存外观设置失败';
  } catch (_) {
    return '保存外观设置失败';
  } finally {
    if (input != null) {
      malloc.free(input);
    }
    if (raw != null && raw != nullptr) {
      try {
        library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(raw);
      } catch (_) {
        // 符号缺失时忽略。
      }
    }
  }
}

// ── 当前主题（`config/appearance.json`）────────────────────────────

typedef _SetC = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _SetDart = Pointer<Utf8> Function(Pointer<Utf8>);

/// 当前主题 id；桥接缺失 / 没存过 = `default`。
String readAppearanceTheme() =>
    _callString('lunac_appearance_theme') ?? 'default';

/// 写主题 id（未知 id 由后端收回到 `default`）。返回 null = 成功，否则是一句原因。
String? setAppearanceTheme(String id) {
  final library = openLunacNative().library;
  if (library == null) {
    return '当前环境不支持切换主题';
  }
  Pointer<Utf8>? input;
  Pointer<Utf8>? raw;
  try {
    final call = library.lookupFunction<_SetC, _SetDart>(
      'lunac_set_appearance_theme',
    );
    input = id.toNativeUtf8();
    raw = call(input);
    if (raw == nullptr) {
      return '保存主题失败';
    }
    final value = jsonDecode(raw.toDartString());
    if (value is! Map<String, dynamic>) {
      return '保存主题失败';
    }
    if (value['ok'] == true) {
      return null;
    }
    final reason = value['error'];
    return reason is String && reason.isNotEmpty ? reason : '保存主题失败';
  } catch (_) {
    return '保存主题失败';
  } finally {
    if (input != null) {
      malloc.free(input);
    }
    if (raw != null && raw != nullptr) {
      try {
        library.lookupFunction<_FreeC, _FreeDart>('lunac_free_string')(raw);
      } catch (_) {
        // 符号缺失时忽略。
      }
    }
  }
}

/// 解析主题包里的颜色写法（v1 允许 `#RRGGBB` / `#RRGGBBAA` / `r,g,b`）。
///
/// [defaultAlpha] 用在 `r,g,b` 三通道写法上（描边默认是半透明白，其余要不透明）。
Color? parseThemeColor(Object? raw, {int defaultAlpha = 0xFF}) {
  if (raw is! String) {
    return null;
  }
  final text = raw.trim();
  if (text.isEmpty) {
    return null;
  }
  if (text.startsWith('#')) {
    var hex = text.substring(1);
    if (hex.length == 3) {
      hex = hex.split('').map((c) => '$c$c').join();
    }
    if (hex.length == 6) {
      final value = int.tryParse(hex, radix: 16);
      return value == null ? null : Color(0xFF000000 | value);
    }
    if (hex.length == 8) {
      // CSS 的 `#RRGGBBAA`。
      final rgb = int.tryParse(hex.substring(0, 6), radix: 16);
      final alpha = int.tryParse(hex.substring(6), radix: 16);
      if (rgb == null || alpha == null) {
        return null;
      }
      return Color((alpha << 24) | rgb);
    }
    return null;
  }
  final parts = text.split(',');
  if (parts.length < 3) {
    return null;
  }
  final channels = <int>[];
  for (final part in parts.take(4)) {
    final value = int.tryParse(part.trim());
    if (value == null || value < 0 || value > 255) {
      return null;
    }
    channels.add(value);
  }
  final alpha = channels.length == 4 ? channels[3] : defaultAlpha;
  return Color.fromARGB(alpha, channels[0], channels[1], channels[2]);
}

/// 把底色往白色抬一档（主题只给了一个 `surface`，卡片 / hover 需要层次）。
Color _lift(Color base, double amount) =>
    Color.lerp(base, const Color(0xFFFFFFFF), amount) ?? base;

/// 把主题包应用到运行时色板；`null` / 默认主题 / 空 tokens = 恢复默认色板。
///
/// 映射口径（v1 `main.ts` 的 CSS 变量与 theme.json 字段一一对应）：
/// `surface` = 窗口底色（v2 的 `background`），并派生 `surface` / `surfaceHover`；
/// `accent` / `text` / `text_dim` / `text_muted` / `border_glass` 直接覆盖。
void applyThemePackage(ThemePackage? theme) {
  if (theme == null || theme.id == 'default' || theme.tokens.isEmpty) {
    LunacColors.reset();
    return;
  }
  final tokens = theme.tokens;
  final background = parseThemeColor(tokens['surface']);
  LunacColors.apply(
    background: background,
    surface: background == null ? null : _lift(background, 0.03),
    surfaceHover: background == null ? null : _lift(background, 0.07),
    accent: parseThemeColor(tokens['accent']),
    text: parseThemeColor(tokens['text']),
    textDim: parseThemeColor(tokens['text_dim']),
    textMuted: parseThemeColor(tokens['text_muted']),
    border: parseThemeColor(tokens['border_glass'], defaultAlpha: 0x1F),
  );
}

/// 在主题包之上叠加自定义配色（v1 `applyAppearance` 的 v2 等价物）。
///
/// 顺序：先按主题包铺底（默认主题 = 默认色板），再叠加用户的四组颜色与文字明暗度。
/// 颜色字段空串 = 该组跟随主题；`tintBase`（「恢复默认主题」）= 四组颜色全部不生效。
/// 底色透明度在 v2 没有桌面透视，按「与默认深底混合」处理。
void applyAppearance(ThemePackage? theme, AppearanceConfig config) {
  applyThemePackage(theme);

  // 底色：用户色（空串 = 主题色），再按底色透明度与原深底混合。
  final base = config.tintBase ? null : parseThemeColor(config.baseColor);
  final source = base ?? LunacColors.background;
  final background =
      Color.lerp(const Color(0xFF16181B), source, config.surfaceAlpha) ??
      source;
  LunacColors.apply(
    background: background,
    surface: _lift(background, 0.03),
    surfaceHover: _lift(background, 0.07),
  );

  if (!config.tintBase) {
    final line = parseThemeColor(config.btnLineColor);
    if (line != null) {
      LunacColors.apply(border: line.withValues(alpha: config.btnLineAlpha));
    }
    final button = parseThemeColor(config.btnBgColor);
    if (button != null) {
      LunacColors.apply(
        surfaceHover: button.withValues(alpha: config.btnBgAlpha),
      );
    }
    final overlay = parseThemeColor(config.overlayColor);
    if (overlay != null) {
      LunacColors.apply(
        divider: overlay.withValues(alpha: config.overlayAlpha),
      );
    }
  }
  if (config.textLight != 0) {
    LunacColors.apply(
      text: _shiftLightness(LunacColors.text, config.textLight),
      textDim: _shiftLightness(LunacColors.textDim, config.textLight),
      textMuted: _shiftLightness(LunacColors.textMuted, config.textLight),
    );
  }
}

/// v1「文字明暗度」：在 HSL 明度上做 ±50% 的偏移（-100…100 → -0.5…0.5）。
Color _shiftLightness(Color color, double amount) {
  final hsl = HSLColor.fromColor(color);
  return hsl
      .withLightness((hsl.lightness + amount / 200).clamp(0.0, 1.0))
      .toColor();
}

/// 启动时应用已保存的主题 + 自定义配色（三处 Flutter 入口在 `runApp` 之前调用）。
///
/// 选中的主题已经被删掉时按默认主题铺底（v1 同样把未知 `themeId` 收回到 `default`）。
void applySavedTheme() {
  final config = readAppearanceConfig();
  final id = config.themeId;
  if (id.isEmpty || id == 'default') {
    applyAppearance(null, config);
    return;
  }
  final themes = readThemes();
  if (themes != null) {
    for (final theme in themes) {
      if (theme.id == id) {
        applyAppearance(theme, config);
        return;
      }
    }
  }
  applyAppearance(null, config);
}

// ── 卫星窗口跟随主窗主题 ──────────────────────────────────────────────

Timer? _appearanceWatch;

/// 外观配置的「指纹」：任何一个会影响界面的字段变了都要重新应用。
///
/// 用字符串比较而不是手写相等判断 —— 字段以后再加一个，这里只需补一项。
String _appearanceSignature() {
  final config = readAppearanceConfig();
  return [
    config.themeId,
    config.baseColor,
    config.surfaceAlpha,
    config.btnLineColor,
    config.btnLineAlpha,
    config.btnBgColor,
    config.btnBgAlpha,
    config.textLight,
    config.overlayColor,
    config.overlayAlpha,
    config.bgImage ?? '',
    config.bgBlur,
    config.bgSaturate,
    config.bgOpacity,
    config.sheen,
    config.tintBase,
  ].join('|');
}

/// 让插件窗 / 附件抽屉这类**独立 engine** 跟上主窗的主题切换。
///
/// v1 所有面板在同一个 WebView 里，设置里换主题立即全界面生效；v2 每个窗口是独立
/// FlutterEngine、独立 Dart 静态区，因此按 [interval] 低频读一次 `config/appearance.json`，
/// 指纹变了就 [applySavedTheme]（内部经 `LunacColors.apply` 自增 `paletteRevision`，
/// 根组件的 `ValueListenableBuilder` 会重建）。
///
/// [signature] / [onChanged] 只给单测注入用；正式路径读真实配置。
/// 桥接缺失（单测 / 无宿主）时指纹恒定，等价空转。
void startAppearanceWatch({
  Duration interval = const Duration(seconds: 1),
  String Function()? signature,
  void Function()? onChanged,
}) {
  _appearanceWatch?.cancel();
  final read = signature ?? _appearanceSignature;
  final apply = onChanged ?? applySavedTheme;
  var last = read();
  _appearanceWatch = Timer.periodic(interval, (_) {
    final next = read();
    if (next == last) {
      return;
    }
    last = next;
    apply();
  });
}

/// 停掉 [startAppearanceWatch] 起的轮询（窗口销毁 / 单测收尾）。
void stopAppearanceWatch() {
  _appearanceWatch?.cancel();
  _appearanceWatch = null;
}
