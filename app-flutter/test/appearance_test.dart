// 平台薄层单测：外观 JSON → Dart 结构 + 主题 tokens → 运行时色板（不加载 DLL）。
import 'dart:ui' show TextDirection;

import 'package:flutter/material.dart' show Color, Directionality, Image, Text;
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/platform/appearance.dart';
import 'package:lunac/theme.dart';

void main() {
  test('system theme json maps accent, dark and source', () {
    final theme = SystemTheme.fromJson(<String, dynamic>{
      'accent': '#374347',
      'dark': true,
      'source': 'registry',
    });
    expect(theme, isNotNull);
    expect(theme!.accent, '#374347');
    expect(theme.dark, isTrue);
    expect(theme.source, 'registry');

    // 缺字段 / 类型不对 ⇒ null（界面显示不可用，而不是画一个假色块）。
    expect(SystemTheme.fromJson(<String, dynamic>{}), isNull);
    expect(
      SystemTheme.fromJson(<String, dynamic>{'dark': 'yes', 'source': 'registry'}),
      isNull,
    );
  });

  test('theme package takes manifest, builtin and resolved fields', () {
    final theme = ThemePackage.fromJson(<String, dynamic>{
      'manifest': <String, dynamic>{
        'id': 'default',
        'name': '默认主题',
        'version': '1.0.0',
        'author': 'Lunac',
        'tokens': <String, dynamic>{'accent': '#C0A0A0'},
      },
      'dir': 'D:/Lunac/themes/default',
      'builtin': true,
      'resolved': <String, dynamic>{
        'background': 'D:/Lunac/themes/default/bg.png',
        'search_pattern': null,
        'icons': <String, dynamic>{'ai-agent': 'D:/Lunac/themes/default/ai.png'},
      },
    });
    expect(theme, isNotNull);
    expect(theme!.id, 'default');
    expect(theme.name, '默认主题');
    expect(theme.version, '1.0.0');
    expect(theme.author, 'Lunac');
    expect(theme.builtin, isTrue);
    expect(theme.dir, 'D:/Lunac/themes/default');
    expect(theme.accent, '#C0A0A0');
    expect(theme.tokens['accent'], '#C0A0A0');
    expect(theme.hasBackground, isTrue);
    expect(theme.background, 'D:/Lunac/themes/default/bg.png');
    expect(theme.iconCount, 1);
  });

  test('minimal theme entry falls back to defaults', () {
    final theme = ThemePackage.fromJson(<String, dynamic>{
      'manifest': <String, dynamic>{'id': 'bare'},
      'dir': 'D:/x/themes/bare',
      'builtin': false,
      'resolved': <String, dynamic>{'icons': <String, dynamic>{}},
    });
    expect(theme, isNotNull);
    expect(theme!.name, 'bare');
    expect(theme.version, '');
    expect(theme.author, '');
    expect(theme.accent, '');
    expect(theme.hasBackground, isFalse);
    expect(theme.iconCount, 0);

    // 没有 id 的条目整条丢弃（Rust 侧保证有 id，这里只是兜底）。
    expect(ThemePackage.fromJson(<String, dynamic>{'manifest': <String, dynamic>{}}), isNull);
    expect(ThemePackage.fromJson(<String, dynamic>{}), isNull);
  });

  test('theme colours accept hex, CSS alpha hex and r,g,b', () {
    expect(parseThemeColor('#CFC8BB'), const Color(0xFFCFC8BB));
    expect(parseThemeColor('#cfc8bb'), const Color(0xFFCFC8BB));
    expect(parseThemeColor('#fff'), const Color(0xFFFFFFFF));
    // CSS 的 #RRGGBBAA（alpha 在末尾）
    expect(parseThemeColor('#11223344'), const Color(0x44112233));
    expect(parseThemeColor('28,26,32'), const Color(0xFF1C1A20));
    expect(parseThemeColor('28,26,32', defaultAlpha: 0x1F), const Color(0x1F1C1A20));
    expect(parseThemeColor(''), isNull);
    expect(parseThemeColor('   '), isNull);
    expect(parseThemeColor('300,0,0'), isNull);
    expect(parseThemeColor('not-a-colour'), isNull);
    expect(parseThemeColor(null), isNull);
  });

  test('applying a theme package changes the palette and reset restores it', () {
    addTearDown(LunacColors.reset);
    final before = LunacColors.accent;

    applyThemePackage(
      const ThemePackage(
        id: 'warm',
        name: 'Warm',
        version: '1',
        author: 't',
        builtin: false,
        dir: '',
        accent: '#C0A0A0',
        hasBackground: false,
        iconCount: 0,
        tokens: {
          'accent': '#C0A0A0',
          'surface': '28,26,32',
          'text': '#EAE2DA',
          'text_dim': '#B2A9A3',
        },
      ),
    );
    expect(LunacColors.accent, const Color(0xFFC0A0A0));
    expect(LunacColors.background, const Color(0xFF1C1A20));
    expect(LunacColors.text, const Color(0xFFEAE2DA));
    expect(LunacColors.textDim, const Color(0xFFB2A9A3));
    // 卡片 / hover 由底色派生，必须比底色亮一档
    expect(LunacColors.surface.r, greaterThan(LunacColors.background.r));

    // 默认主题 / null = 恢复默认色板
    applyThemePackage(null);
    expect(LunacColors.accent, before);
    expect(LunacColors.background, const Color(0xFF16181B));
  });

  test('appearance config reads background image fields and tintBase', () {
    final config = AppearanceConfig.fromJson(<String, dynamic>{
      'bgImage': 'D:/pics/bg.png',
      'bgBlur': 12,
      'bgSaturate': 120,
      'bgOpacity': 0.4,
      'sheen': 0.3,
      'tintBase': true,
    });
    expect(config.bgImage, 'D:/pics/bg.png');
    expect(config.bgBlur, 12);
    expect(config.bgSaturate, 120);
    expect(config.bgOpacity, 0.4);
    expect(config.sheen, 0.3);
    expect(config.tintBase, isTrue);

    // copyWith 里 bgImage: null = 保持原值，clearBgImage = 真正清除
    expect(config.copyWith().bgImage, 'D:/pics/bg.png');
    expect(config.copyWith(clearBgImage: true).bgImage, isNull);
    expect(config.copyWith(bgImage: 'D:/other.png').bgImage, 'D:/other.png');
  });

  test('tintBase ignores custom colours but custom colours apply without it', () {
    addTearDown(LunacColors.reset);

    applyAppearance(
      null,
      const AppearanceConfig(baseColor: '#FF0000', tintBase: true),
    );
    expect(
      LunacColors.background,
      isNot(const Color(0xFFFF0000)),
      reason: '恢复默认主题时用户底色不生效',
    );

    applyAppearance(null, const AppearanceConfig(baseColor: '#FF0000'));
    expect(LunacColors.background, const Color(0xFFFF0000));
  });

  testWidgets('appearance watch re-applies only when the signature changes', (
    tester,
  ) async {
    var signature = 'a';
    var applied = 0;
    startAppearanceWatch(
      interval: const Duration(milliseconds: 100),
      signature: () => signature,
      onChanged: () => applied++,
    );
    addTearDown(stopAppearanceWatch);

    await tester.pump(const Duration(milliseconds: 250));
    expect(applied, 0, reason: '指纹没变时不要反复重建');

    signature = 'b';
    await tester.pump(const Duration(milliseconds: 100));
    expect(applied, 1);

    await tester.pump(const Duration(milliseconds: 200));
    expect(applied, 1, reason: '同一次变化只应用一次');

    // 轮询是常驻 Timer：测试结束前必须停掉，否则 framework 报 pending timer。
    stopAppearanceWatch();
  });

  testWidgets('backdrop draws nothing extra without an image', (tester) async {
    await tester.pumpWidget(
      const Directionality(
        textDirection: TextDirection.ltr,
        child: AppearanceBackdrop(
          imagePath: null,
          blur: 4,
          saturate: 92,
          opacity: 0.5,
          sheen: 0.2,
          child: Text('content'),
        ),
      ),
    );
    expect(find.text('content'), findsOneWidget);
    expect(find.byType(Image), findsNothing);
  });
}
