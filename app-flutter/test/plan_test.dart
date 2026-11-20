// §待办 3-28.3 计划步骤的纯 Dart 测试：解析 / 编辑后的正文 / 留档降级。
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/platform/plans.dart';
import 'package:lunac/search/chat_model.dart';

void main() {
  test('plan steps recognise bullets and numbering but skip headings', () {
    final steps = planStepsOf(
      '# 实现计划\n\n## 第一步\n- 读取配置\n2. 修改入口\n\n说明段落。\n',
    );
    expect(steps.map((step) => step.title), ['读取配置', '修改入口']);
    expect(steps.every((step) => step.enabled), isTrue, reason: '默认全部保留');
  });

  test('plans without bullets fall back to every non-empty line', () {
    // 模型有时直接写正文：不能因为「没有项目符号」就给出一个没法编辑的空卡。
    final steps = planStepsOf('先读代码\n\n再改入口\n');
    expect(steps.map((step) => step.title), ['先读代码', '再改入口']);
  });

  test('edited plan keeps only enabled steps and renumbers them', () {
    final steps = planStepsOf('- 读取配置\n- 修改入口\n- 跑测试');
    steps[0].enabled = false;
    expect(
      editedPlanText(steps, note: ' 别动数据库 '),
      '1. 修改入口\n2. 跑测试\n\n补充说明：别动数据库',
    );
    // 没有备注时不留下多余空行。
    expect(editedPlanText(steps), '1. 修改入口\n2. 跑测试');
    // 全关掉 ⇒ 空串（面板据此禁用批准键）。
    for (final step in steps) {
      step.enabled = false;
    }
    expect(editedPlanText(steps), '');
  });

  test('archive stays quiet without the native bridge', () {
    // 留档是事后取证：桥接缺失 / 写盘失败都只记宿主日志，绝不抛到界面上。
    expect(
      archivePlan(original: '- a', edited: '1. a', note: ''),
      isNull,
    );
  });
}
