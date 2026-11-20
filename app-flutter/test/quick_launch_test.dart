// 快捷启动平台薄层的数据模型测试（不加载 DLL）。
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/platform/quick_launch.dart';

void main() {
  test('fromJson 读取完整字段', () {
    final app = CustomApp.fromJson(const {
      'name': '记事本',
      'path': r'C:\Windows\notepad.exe',
      'source': 'custom',
    });
    expect(app, isNotNull);
    expect(app!.name, '记事本');
    expect(app.path, r'C:\Windows\notepad.exe');
    expect(app.source, 'custom');
  });

  test('fromJson 缺字段返回 null', () {
    expect(CustomApp.fromJson(const {'path': r'C:\Windows\notepad.exe'}), isNull);
    expect(CustomApp.fromJson(const {'name': '记事本'}), isNull);
    expect(
      CustomApp.fromJson(const {'name': '记事本', 'path': ''}),
      isNull,
    );
    // source 缺省时按 custom 处理（旧注册表也兼容）。
    final app = CustomApp.fromJson(const {
      'name': '记事本',
      'path': r'C:\Windows\notepad.exe',
    });
    expect(app?.source, 'custom');
  });

  test('fromJson 非法类型返回 null', () {
    expect(
      CustomApp.fromJson(const {'name': 1, 'path': r'C:\Windows\notepad.exe'}),
      isNull,
    );
    expect(
      CustomApp.fromJson(const {'name': '记事本', 'path': 2}),
      isNull,
    );
    expect(
      CustomApp.fromJson(const {
        'name': '记事本',
        'path': r'C:\Windows\notepad.exe',
        'source': 3,
      })?.source,
      'custom',
    );
  });
}
