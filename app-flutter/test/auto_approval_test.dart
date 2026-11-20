// 命令审批的自动放行判定（纯 Dart；不碰 FFI / 协议）。
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/search/auto_approval.dart';

void main() {
  test('auto 放行普通命令，但危险 / 凭据 / 问答永远问', () {
    expect(
      shouldAutoApprove(
        toolName: 'Cmd',
        analysis: const {'readonly': false},
        runMode: 'auto',
      ),
      isTrue,
      reason: '自动档的规范语义就是不再询问',
    );
    expect(
      shouldAutoApprove(
        toolName: 'Cmd',
        analysis: const {
          'dangerous': ['shutdown /s'],
        },
        runMode: 'auto',
      ),
      isFalse,
      reason: '危险命令任何档位都要人看',
    );
    expect(
      shouldAutoApprove(
        toolName: 'Write',
        analysis: const {
          'secrets': [
            {'rule': 'aws-key', 'line': 3},
          ],
        },
        runMode: 'auto',
      ),
      isFalse,
      reason: '命中凭据不自动放行',
    );
    expect(
      shouldAutoApprove(
        toolName: 'AskUserQuestion',
        analysis: null,
        runMode: 'auto',
      ),
      isFalse,
      reason: '自动放行等于替用户回答',
    );
    expect(
      shouldAutoApprove(toolName: 'ExitPlanMode', analysis: null, runMode: 'auto'),
      isFalse,
      reason: '自动放行等于没人读计划就开工',
    );
    expect(
      shouldAutoApprove(toolName: 'Screenshot', analysis: null, runMode: 'auto'),
      isFalse,
      reason: '抓的是用户屏幕（隐私），自动档也不该静默放行',
    );
    // §待办 3-28.15：RunScript 跑的是模型写的代码 —— 自动档也必须让人看脚本。
    expect(
      shouldAutoApprove(toolName: 'RunScript', analysis: null, runMode: 'auto'),
      isFalse,
      reason: '批了脚本 ≠ 批了它做的一切，裁决点不能被自动档省掉',
    );
  });

  test('manual 全部问，allowlist 只放行可证只读命令', () {
    expect(
      shouldAutoApprove(
        toolName: 'Cmd',
        analysis: const {'readonly': true},
        runMode: 'manual',
      ),
      isFalse,
    );
    expect(
      shouldAutoApprove(
        toolName: 'Cmd',
        analysis: const {'readonly': true},
        runMode: 'allowlist',
      ),
      isTrue,
    );
    expect(
      shouldAutoApprove(
        toolName: 'Cmd',
        analysis: const {'readonly': false},
        runMode: 'allowlist',
      ),
      isFalse,
    );
    expect(
      shouldAutoApprove(toolName: 'Cmd', analysis: null, runMode: 'allowlist'),
      isFalse,
      reason: '缺 analysis 按不放行（fail-closed）',
    );
    expect(
      shouldAutoApprove(toolName: 'Write', analysis: const {}, runMode: 'allowlist'),
      isFalse,
      reason: 'v2 暂无用户工具白名单，写类一律询问',
    );
  });
}
