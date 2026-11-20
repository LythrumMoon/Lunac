// 平台薄层单测：提权子进程参数识别（不依赖 DLL / 不触碰真实注册表）。
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/platform/auto_start.dart';

void main() {
  test('elevated flag parsing only accepts the exact switch', () {
    expect(
      parseElevatedAction(['lunac.exe', '--lunac-auto-start-task=create']),
      'create',
    );
    expect(parseElevatedAction(['--lunac-auto-start-task=delete']), 'delete');
    // 普通启动 / 后台启动不是提权子进程。
    expect(parseElevatedAction(['--background']), isNull);
    expect(parseElevatedAction(<String>[]), isNull);
    // 空动作不认（否则会拉起一个什么也不做的提权进程）。
    expect(parseElevatedAction(['--lunac-auto-start-task=']), isNull);
  });

  test('mechanism label explains what the user will experience', () {
    const task = AutoStartInfo(
      enabled: true,
      mechanism: 'task',
      stale: false,
      dev: false,
      supported: true,
    );
    const none = AutoStartInfo(
      enabled: false,
      mechanism: 'none',
      stale: false,
      dev: false,
      supported: true,
    );
    expect(task.mechanismLabel, contains('较快'));
    expect(none.mechanismLabel, '未开启');
  });
}
