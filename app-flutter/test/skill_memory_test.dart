// 技能级记忆（§待办 3-28.10 ①）的纯数据层测试：不碰 FFI。
//
// 覆盖两件事：
//   · `SkillMemoryEntry.fromJson` 的字段映射与坏数据回落；
//   · `ChatSettings.fromJson` 把 `skillMemory` / `ptcMode` 两个开关读进来（缺省关）。
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/platform/agent.dart';
import 'package:lunac/platform/memory.dart';

void main() {
  test('skill memory entries map fields and drop bad rows', () {
    final entry = SkillMemoryEntry.fromJson({
      'key': 'code-review',
      'chars': 128,
      'updatedAt': 1730000000000,
      'path': r'D:\Lunac\ModuleData\memory\skills\code-review\MEMORY.md',
    });
    expect(entry, isNotNull);
    expect(entry!.key, 'code-review');
    expect(entry.chars, 128);
    expect(entry.updatedAt, 1730000000000);
    expect(entry.path, contains('skills'));

    // 缺 key / key 不是字符串 / 不是对象 ⇒ 丢掉（不造半条假数据）。
    expect(SkillMemoryEntry.fromJson({'chars': 1}), isNull);
    expect(SkillMemoryEntry.fromJson({'key': ''}), isNull);
    expect(SkillMemoryEntry.fromJson({'key': 7}), isNull);
    expect(SkillMemoryEntry.fromJson('nope'), isNull);

    // 可选字段坏掉时按 0 / 空串回落，不整条丢弃。
    final loose = SkillMemoryEntry.fromJson({'key': 'demo', 'chars': 'many'});
    expect(loose, isNotNull);
    expect(loose!.chars, 0);
    expect(loose.updatedAt, 0);
    expect(loose.path, '');
  });

  test('chat settings carry the skill-memory and PTC toggles', () {
    final off = ChatSettings.fromJson({});
    expect(off.skillMemory, isFalse, reason: '技能级记忆缺省关');
    expect(off.ptcMode, isFalse, reason: 'PTC 缺省关');

    final on = ChatSettings.fromJson({
      'skillMemory': true,
      'ptcMode': true,
    });
    expect(on.skillMemory, isTrue);
    expect(on.ptcMode, isTrue);

    // 非布尔值回落缺省，不抛。
    final bad = ChatSettings.fromJson({'skillMemory': 'yes', 'ptcMode': 1});
    expect(bad.skillMemory, isFalse);
    expect(bad.ptcMode, isFalse);
  });
}
