// 后台任务队列的 Dart 侧纯解析（不碰 FFI）。
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/platform/task_queue.dart';

void main() {
  test('task items map the queue contract and label states', () {
    final item = TaskItem.fromJson(const {
      'id': 'task_1',
      'prompt': '整理下载目录',
      'status': 'running',
      'round': 2,
      'maxRounds': 5,
      'sessionId': 'sess_1',
      'summary': '',
      'error': '',
      'createdAt': 1700000000000,
      'updatedAt': 1700000001000,
    });
    expect(item.id, 'task_1');
    expect(item.prompt, '整理下载目录');
    expect(item.isRunning, isTrue);
    expect(item.isPending, isFalse);
    expect(item.statusLabel, '运行中');
    expect(item.round, 2);
    expect(item.maxRounds, 5);
    expect(item.sessionId, 'sess_1');
  });

  test('missing status falls back to pending and unknown states stay verbatim', () {
    final missing = TaskItem.fromJson(const {'id': 'task_2', 'prompt': 'x'});
    expect(missing.status, 'pending', reason: '契约里缺省就是排队中');
    expect(missing.statusLabel, '排队中');

    final unknown = TaskItem.fromJson(const {
      'id': 'task_3',
      'status': 'paused',
    });
    expect(
      unknown.statusLabel,
      'paused',
      reason: '认不出的状态原样显示，不吞信息也不编中文',
    );
  });
}
