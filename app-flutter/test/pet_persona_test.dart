// 桌宠人格 / 三份列表的 JSON 解析单测（不加载 DLL）。
import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/platform/pet.dart';

void main() {
  test('persona json maps text, path and max chars', () {
    final persona = PetPersona.fromJson(<String, dynamic>{
      'ok': true,
      'path': r'D:\Lunac\config\pet-persona.md',
      'text': '你是 Lunac 的桌面宠物。',
      'maxChars': 8000,
      'error': '',
    });
    expect(persona.ok, isTrue);
    expect(persona.path, r'D:\Lunac\config\pet-persona.md');
    expect(persona.text, '你是 Lunac 的桌面宠物。');
    expect(persona.maxChars, 8000);

    // 桥接缺失 / 坏形状：ok=false + 默认上限，界面据此显示「不可用」。
    expect(PetPersona.fromJson(null).ok, isFalse);
    expect(PetPersona.fromJson(<String, dynamic>{}).maxChars, 8000);
  });

  test('memory list json maps entries and limits', () {
    final list = PetMemoryList.fromJson(<String, dynamic>{
      'ok': true,
      'entries': ['喜欢吃草莓', '第二行补充'],
      'maxEntryChars': 2000,
      'maxTotalChars': 6000,
      'error': '',
    });
    expect(list.entries, ['喜欢吃草莓', '第二行补充']);
    expect(list.maxEntryChars, 2000);
    expect(list.maxTotalChars, 6000);
    expect(list.error, isEmpty);

    // 缺字段 / 形状不对：空列表 + 默认上限，不抛异常。
    final broken = PetMemoryList.fromJson(<String, dynamic>{'entries': 3});
    expect(broken.entries, isEmpty);
    expect(broken.maxEntryChars, 2000);
    expect(broken.maxTotalChars, 6000);
  });

  test('memory kinds match the C ABI ids', () {
    // Rust 侧 `pet_list_path` 只认这三个字符串；改名字必须两边一起改。
    expect(PetMemoryKind.memory.name, 'memory');
    expect(PetMemoryKind.growth.name, 'growth');
    expect(PetMemoryKind.suggestions.name, 'suggestions');
  });

  test('config keeps per-model expressions and debug params', () {
    final config = PetConfig.fromJson(<String, dynamic>{
      'model': r'D:\pet\m.model3.json',
      'expressions': {r'D:\pet\m.model3.json': 'smile', '': 'x'},
      'params': {
        r'D:\pet\m.model3.json': {'ParamAngleX': 12.5, 'ParamBreath': 3},
        'broken': 'nope',
      },
    });
    expect(config.expressions, {r'D:\pet\m.model3.json': 'smile'});
    expect(config.params[r'D:\pet\m.model3.json'], {
      'ParamAngleX': 12.5,
      'ParamBreath': 3,
    });
    expect(config.params.containsKey('broken'), isFalse);
    // 缺字段：空表，不抛。
    final bare = PetConfig.fromJson(<String, dynamic>{'model': 'm'});
    expect(bare.expressions, isEmpty);
    expect(bare.params, isEmpty);
  });

  test('proactive config clamps numbers and keeps the daily counter', () {
    final today = ProactiveConfig.todayKey();
    final config = PetConfig.fromJson(<String, dynamic>{
      'proactive': {
        'enabled': true,
        'intervalMin': 0,
        'dailyCap': -5,
        'screenPollSec': 9999,
        'screenMinGapSec': 1,
        'screenDiff': 0,
        'date': today,
        'used': 3.7,
      },
    });
    // 越界夹到区间（与 v1 `clampInt` 同口径）；`-5` 夹到 0 = 不限。
    expect(config.proactive.enabled, isTrue);
    expect(config.proactive.intervalMin, 1);
    expect(config.proactive.dailyCap, 0);
    expect(config.proactive.screenPollSec, 600);
    expect(config.proactive.screenMinGapSec, 30);
    expect(config.proactive.screenDiff, 1);
    expect(config.proactive.used, 3);
    expect(config.proactive.usedToday(), 3);
    // cap = 0 ⇒ 不限次数，永远不到顶。
    expect(config.proactive.capReached(), isFalse);
  });

  test('proactive counter only counts today', () {
    final stale = ProactiveConfig.fromJson(<String, dynamic>{
      'dailyCap': 5,
      'date': '2026-01-01',
      'used': 5,
    });
    expect(stale.usedToday(), 0);
    expect(stale.capReached(), isFalse);

    final today = ProactiveConfig.fromJson(<String, dynamic>{
      'dailyCap': 5,
      'date': ProactiveConfig.todayKey(),
      'used': 5,
    });
    expect(today.usedToday(), 5);
    expect(today.capReached(), isTrue);
  });

  test('motion template builds constant curves and respects physics opt-in', () {
    final coreIds = <String, dynamic>{
      'parameters': ['ParamAngleX', 'ParamHair'],
      'parameterRanges': {
        'ParamAngleX': {'min': -30, 'max': 30, 'def': 2},
      },
    };
    final without = buildPetMotionTemplate(
      coreIds: coreIds,
      physicsOutputs: {'ParamHair'},
      includePhysics: false,
    );
    expect(without, isNotNull);
    final decoded = jsonDecode(without!) as Map<String, dynamic>;
    // 默认排除物理输出 ⇒ 只剩 ParamAngleX 一条常数曲线（值 = 默认值）。
    expect((decoded['Meta'] as Map)['CurveCount'], 1);
    final curve = (decoded['Curves'] as List).single as Map;
    expect(curve['Id'], 'ParamAngleX');
    expect(curve['Segments'], [0, 2.0, 0, 1, 2.0]);

    final withPhysics = buildPetMotionTemplate(
      coreIds: coreIds,
      physicsOutputs: {'ParamHair'},
      includePhysics: true,
    )!;
    expect(((jsonDecode(withPhysics) as Map)['Meta'] as Map)['CurveCount'], 2);

    // 拿不到运行时参数表 ⇒ null（调用方提示先显示桌宠）。
    expect(
      buildPetMotionTemplate(
        coreIds: const {'parameters': <String>[]},
        physicsOutputs: const {},
        includePhysics: false,
      ),
      isNull,
    );
  });

  test('motion file json keeps display names and disk file names', () {
    final files = PetMotionFile.listFromJson(<Object?>[
      {
        'path': r'D:\m\lunac-live2d\motions\a.motion3.json',
        'name': 'a',
      },
      {'path': r'D:\m\b.motion3.json'},
    ]);
    expect(files.first.name, 'a');
    expect(files.first.fileName, 'a.motion3.json');
    // 缺 name ⇒ 用文件名词干兜底。
    expect(files.last.name, 'b');
  });

  test('model info maps expression names and motion groups', () {
    final info = PetModelInfo.fromJson(<String, dynamic>{
      'ok': true,
      'version': 4,
      'textures': 1,
      'motions': 2,
      'expressions': 2,
      'expressionNames': ['smile', ' ', 'sad', 3],
      'motionGroups': ['Idle', 'Tap'],
      'reason': 'Idle / Tap',
    });
    expect(info.ok, isTrue);
    expect(info.expressionNames, ['smile', 'sad'], reason: '空名与非字符串要丢掉');
    expect(info.motionGroups, ['Idle', 'Tap']);
    // 缺字段：空表。
    expect(PetModelInfo.fromJson(<String, dynamic>{}).expressionNames, isEmpty);
  });

  test('hotkey json keeps action payloads and skips entries without keys', () {
    final hotkeys = PetHotkey.listFromJson(<Object?>[
      {
        'id': 'h1',
        'name': '微笑',
        'keys': 'Ctrl+Numpad1',
        'action': {
          'type': 'expression',
          'target': 'smile',
          'mode': 'set',
          'file': r'D:\m\smile.exp3.json',
        },
      },
      {
        'id': 'h2',
        'name': '挥手',
        'keys': 'Shift+Numpad2',
        'action': {'type': 'motion', 'group': 'Tap', 'index': 3},
      },
      // 没有 keys ⇒ v1 同口径视作坏数据丢掉。
      {'id': 'h3', 'name': '坏条目', 'action': {'type': 'physics'}},
    ]);
    expect(hotkeys, hasLength(2));
    expect(hotkeys.first.actionType, 'expression');
    expect(hotkeys.first.actionLabel, '表情 smile（开启）');
    expect(hotkeys.last.actionLabel, '动作 Tap #3');
    // 落盘必须原样带回 action（AI / 用户手改过的字段不能被抹掉）。
    expect(hotkeys.first.toJson()['action'], containsPair('file', r'D:\m\smile.exp3.json'));
  });

  test('hotkey action labels cover every v1 action type', () {
    String label(Map<String, Object?> action) =>
        PetHotkey(id: 'x', name: 'n', keys: 'k', action: action).actionLabel;
    expect(label({'type': 'chain', 'id': 'c1'}), '动作链 c1');
    expect(label({'type': 'expression', 'target': 'sad', 'mode': 'clear'}), '表情 sad（关闭）');
    expect(label({'type': 'param', 'id': 'ParamAngleX', 'value': 30, 'mode': 'add'}),
        '参数 ParamAngleX += 30');
    expect(label({'type': 'artmesh', 'id': 'ArtMesh1', 'value': 0}), 'ArtMesh ArtMesh1 = 0');
    expect(label({'type': 'artmeshColor', 'id': 'ArtMesh1', 'color': '#ff0000'}),
        '染色 ArtMesh1 #ff0000');
    expect(label({'type': 'part', 'id': 'Part1', 'value': 1}), 'Part Part1 = 1');
    expect(label({'type': 'physics', 'enabled': false}), '物理：关');
    expect(label(<String, Object?>{}), '（无动作）');
  });

  test('default hotkey keys stack modifiers in v1 order', () {
    expect(defaultPetHotkeyKeys(0), 'Ctrl+Numpad1');
    expect(defaultPetHotkeyKeys(8), 'Ctrl+Numpad9');
    expect(defaultPetHotkeyKeys(9), 'Shift+Numpad1');
    expect(defaultPetHotkeyKeys(63), 'Numpad1', reason: '超出 7 层后退回裸数字键');
    expect(defaultPetHotkeyKeys(-1), 'Ctrl+Numpad1');
  });

  test('hotkey read and write degrade without the native bridge', () {
    // 单测没有 lunac_core.dll：读回空表、写回一句人话，都不抛异常。
    expect(readPetHotkeys(r'D:\m\a.model3.json'), isEmpty);
    final written = writePetHotkeys(r'D:\m\a.model3.json', const []);
    expect(written.path, isNull);
    expect(written.error, isNotNull);
  });

  test('voice status maps config, assets and the honest reason', () {
    final voice = PetVoiceStatus.fromJson(<String, dynamic>{
      'enabled': false,
      'engine': 'gpt_sovits',
      'serverUrl': 'http://127.0.0.1:9880',
      'sampleSteps': 4,
      'textLang': 'all_zh',
      'referenceAudio': r'D:\ref.wav',
      'referenceText': '这是参考',
      'speed': 1.4,
      'volume': 0.35,
      'numSteps': 8,
      'numThreads': 4,
      'assetsReady': true,
      'serverReady': true,
      'assetDir': r'D:\Lunac\Modules\pet\gsv',
      'missingAssets': [''],
      'speedMin': 0.5,
      'speedMax': 2.0,
      'volumeMin': 0.0,
      'volumeMax': 1.0,
      'reason': 'ready',
      'reasonText': 'GPT-SoVITS 已就绪',
      'canSpeak': true,
      'path': r'D:\Lunac\config\pet-voice.json',
    })!;
    expect(voice.enabled, isFalse);
    expect(voice.engine, 'gpt_sovits');
    expect(voice.serverUrl, 'http://127.0.0.1:9880');
    expect(voice.sampleSteps, 4);
    expect(voice.speed, 1.4);
    expect(voice.volume, 0.35);
    expect(voice.assetsReady, isTrue);
    expect(voice.serverReady, isTrue);
    expect(voice.missingAssets, isEmpty, reason: '空串要丢掉');
    expect(voice.reason, 'ready');
    expect(voice.canSpeak, isTrue);
    expect(voice.ok, isTrue);

    // 缺字段：回落默认值（GPT-SoVITS / 9880 / 4 步）。
    final empty = PetVoiceStatus.fromJson(<String, dynamic>{})!;
    expect(empty.enabled, isTrue);
    expect(empty.engine, 'gpt_sovits');
    expect(empty.serverUrl, 'http://127.0.0.1:9880');
    expect(empty.sampleSteps, 4);
    expect(empty.textLang, 'all_zh');
    expect(empty.speed, 1.0);
    expect(empty.volume, 1.0);
    expect(PetVoiceStatus.fromJson(null), isNull);
  });

  test('voice read degrades without the native bridge', () {
    final voice = readPetVoice();
    expect(voice.ok, isFalse);
    expect(voice.error, contains('未加载'));
    final saved = savePetVoice(const {'enabled': true});
    expect(saved.ok, isFalse);
    expect(saved.error, isNotNull);
  });
}
