// 技能管理的数据面：清单 / 安装回执的字段映射（§待办 3-28.9 ②③）。
//
// 真正的哈希校验与权限 diff 判定在 Rust（`app-flutter/rust/src/skills_admin.rs`），
// 这里只钉住「宿主回执 → Dart 对象」这一层，别让界面把 `hashMismatch` 读丢。
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/platform/skills_admin.dart';

void main() {
  test('skill entries carry sha256 and allowed tools', () {
    final entry = SkillEntry.fromJson(const {
      'key': 'code-review',
      'name': '代码审查',
      'sha256': 'abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789',
      'allowedTools': ['Read', 'Grep'],
      'bytes': 42,
      'valid': true,
    });
    expect(entry.sha256, startsWith('abcdef012345'));
    expect(entry.hashShort, 'abcdef012345');
    expect(entry.allowedTools, ['Read', 'Grep']);

    // 坏字段不抛：缺 sha256 / allowedTools 时是空串 / 空表。
    final bare = SkillEntry.fromJson(const {'key': 'x', 'valid': false});
    expect(bare.sha256, isEmpty);
    expect(bare.allowedTools, isEmpty);
  });

  test('install result decodes hash and permission gates', () {
    final blocked = SkillResult.fromRaw(
      '{"ok":false,"key":"a","hashMismatch":true,'
      '"sha256":"1111111111111111111111111111111111111111111111111111111111111111",'
      '"error":"内容哈希不符"}',
    );
    expect(blocked.ok, isFalse);
    expect(blocked.hashMismatch, isTrue);
    expect(blocked.permissionBlocked, isFalse);
    expect(blocked.hashShort, '111111111111');
    expect(blocked.permissions, isNull);

    final expand = SkillResult.fromRaw(
      '{"ok":false,"permissionBlocked":true,"permissions":{'
      '"tools":["Read","Write"],"unrestricted":false,'
      '"added":["Write"],"removed":[],"broadens":true}}',
    );
    expect(expand.permissionBlocked, isTrue);
    expect(expand.permissions?.added, ['Write']);
    expect(expand.permissions?.broadens, isTrue);
    expect(expand.permissions?.summary, 'Read、Write');

    // 不限制时 summary 要如实说「任意工具」，不能显示空串。
    final open = SkillResult.fromRaw(
      '{"ok":true,"permissions":{"tools":[],"unrestricted":true}}',
    );
    expect(open.permissions?.summary, '任意工具');
  });
}
