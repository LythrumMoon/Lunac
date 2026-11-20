// 白箱边界账本（W0）的纯函数：原因码 / 目标 / 文案 / 行形状。
//
// 不碰 FFI：`describeBoundary` 与 `shouldAutoApprove` 是同一套判定的两面，
// 这里用矩阵钉住「自动放行时原因码只可能是 RUN_MODE_AUTO / AUTO_READONLY」。
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/platform/security_ledger.dart';
import 'package:lunac/search/auto_approval.dart';

void main() {
  test('原因码优先级：危险 > 不可判定 > 凭据 > 计划 > 提问 > 运行方式', () {
    expect(
      describeBoundary(
        toolName: 'Cmd',
        analysis: const {
          'dangerous': ['递归强制删除'],
          'opaque': ['命令词是变量'],
          'secrets': [
            {'rule': 'AWS access key', 'line': 3},
          ],
        },
        runMode: 'auto',
      ),
      (code: 'DANGER_RULE', detail: '递归强制删除'),
    );
    expect(
      describeBoundary(
        toolName: 'Cmd',
        analysis: const {
          'opaque': ['编码执行'],
        },
        runMode: 'allowlist',
      ),
      (code: 'OPAQUE_COMMAND', detail: '编码执行'),
      reason: '自动档不吃这条（那一档本来就不弹卡），所以用白名单档钉住',
    );
    expect(
      describeBoundary(
        toolName: 'Cmd',
        analysis: const {
          'opaque': ['编码执行'],
        },
        runMode: 'auto',
      ),
      (code: 'RUN_MODE_AUTO', detail: ''),
      reason: '自动档不吃 opaque（v1 classifyRequest ② 的唯一例外）',
    );
    expect(
      describeBoundary(
        toolName: 'Write',
        analysis: const {
          'secrets': [
            {'rule': 'AWS access key', 'line': 3},
          ],
        },
        runMode: 'auto',
      ),
      (code: 'SECRET_IN_PAYLOAD', detail: 'AWS access key（第 3 行）'),
      reason: '写入类分析给的是 {rule,line} 对象，不能当成「没有命中」',
    );
    expect(
      describeBoundary(toolName: 'ExitPlanMode', analysis: null, runMode: 'auto'),
      (code: 'PLAN_APPROVAL', detail: ''),
    );
    expect(
      describeBoundary(
        toolName: 'AskUserQuestion',
        analysis: null,
        runMode: 'auto',
      ),
      (code: 'ASK_QUESTION', detail: ''),
    );
    expect(
      describeBoundary(
        toolName: 'Cmd',
        analysis: const {'readonly': false},
        runMode: 'auto',
      ),
      (code: 'RUN_MODE_AUTO', detail: ''),
    );
  });

  test('白名单档：只有可证只读命令记 AUTO_READONLY，其余 ASK_DEFAULT', () {
    expect(
      describeBoundary(
        toolName: 'Cmd',
        analysis: const {'readonly': true},
        runMode: 'allowlist',
      ),
      (code: 'AUTO_READONLY', detail: ''),
    );
    expect(
      describeBoundary(
        toolName: 'Cmd',
        analysis: const {'readonly': false},
        runMode: 'allowlist',
      ),
      (code: 'ASK_DEFAULT', detail: ''),
    );
    expect(
      describeBoundary(toolName: 'Cmd', analysis: null, runMode: 'allowlist'),
      (code: 'ASK_DEFAULT', detail: ''),
      reason: '缺 analysis 一律 fail-closed',
    );
    expect(
      describeBoundary(
        toolName: 'Write',
        analysis: const {'readonly': true},
        runMode: 'allowlist',
      ),
      (code: 'ASK_DEFAULT', detail: ''),
      reason: 'v2 暂无用户工具白名单',
    );
    expect(
      describeBoundary(
        toolName: 'Cmd',
        analysis: const {'readonly': true},
        runMode: 'manual',
      ),
      (code: 'ASK_DEFAULT', detail: ''),
      reason: '手动档不看 readonly',
    );
  });

  test('原因码与自动放行判定不矛盾（同一矩阵）', () {
    const cases = <({String tool, Map<String, dynamic>? analysis, String mode})>[
      (tool: 'Cmd', analysis: null, mode: 'manual'),
      (tool: 'Cmd', analysis: {'readonly': true}, mode: 'allowlist'),
      (tool: 'Cmd', analysis: {'readonly': false}, mode: 'allowlist'),
      (tool: 'Cmd', analysis: {'readonly': true}, mode: 'auto'),
      (tool: 'Cmd', analysis: {'dangerous': ['x']}, mode: 'auto'),
      (tool: 'Cmd', analysis: {'opaque': ['y']}, mode: 'auto'),
      (tool: 'Write', analysis: null, mode: 'auto'),
      (
        tool: 'Write',
        analysis: {
          'secrets': [
            {'rule': 'k', 'line': 1},
          ],
        },
        mode: 'auto',
      ),
      (tool: 'ExitPlanMode', analysis: null, mode: 'auto'),
      (tool: 'AskUserQuestion', analysis: null, mode: 'auto'),
    ];
    const autoCodes = {'RUN_MODE_AUTO', 'AUTO_READONLY'};
    for (final item in cases) {
      final auto = shouldAutoApprove(
        toolName: item.tool,
        analysis: item.analysis,
        runMode: item.mode,
      );
      final why = describeBoundary(
        toolName: item.tool,
        analysis: item.analysis,
        runMode: item.mode,
      );
      expect(
        autoCodes.contains(why.code),
        auto,
        reason:
            '${item.tool}/${item.mode}/${item.analysis} 判定=$auto 原因码=${why.code}',
      );
      expect(kSecurityReasonCodes, contains(why.code));
    }
  });

  test('target 取值顺序：命令 > 文件路径 > 路径 / URL > 工具名', () {
    expect(
      securityTarget('Cmd', const {'command': 'Get-Date', 'path': 'D:\\x'}),
      'Get-Date',
    );
    expect(
      securityTarget('Write', const {'file_path': 'D:\\a.txt'}),
      'D:\\a.txt',
    );
    expect(securityTarget('Preview', const {'url': 'https://x'}), 'https://x');
    expect(securityTarget('Read', const {'file_path': '  '}), 'Read');
    expect(securityTarget('Bash', null), 'Bash');
  });

  test('每个原因码都有中文文案（不许落到「其他判定」）', () {
    for (final code in kSecurityReasonCodes) {
      expect(
        reasonTextFor(code),
        isNotEmpty,
        reason: '$code 必须有文案',
      );
      expect(
        reasonTextFor(code, 'detail'),
        isNotEmpty,
        reason: '$code 带 detail 也要有文案',
      );
    }
    expect(reasonTextFor('MADE_UP'), '其他判定');
    expect(reasonTextFor('DANGER_RULE', '递归强制删除'), '命中危险规则：递归强制删除');
    expect(reasonTextFor('USER_DENY'), '用户点了拒绝');
    // 枚举与 docs/sandbox-research.md §6.2 一致（25 条，无重复）。
    expect(kSecurityReasonCodes.length, 25);
    expect(kSecurityReasonCodes.toSet().length, kSecurityReasonCodes.length);
  });

  test('预演分析在无桥接时如实报错（不冒充「无风险」）', () async {
    final result = await analyzeCommand('git status');
    expect(result.analysis, isNull);
    expect(result.error, isNotNull);
    expect(result.error, contains('不可用'));
  });

  test('行形状：解析宿主落盘的一行，回写载荷不含 v/ts', () {
    final entry = SecurityEntry.fromJson(const {
      'v': 1,
      'ts': '2026-11-15T05:12:33Z',
      'session': 'sess-1',
      'actor': 'agent',
      'tool': 'Cmd',
      'target': 'powershell -c Get-Date',
      'decision': 'ask',
      'reasonCode': 'ASK_DEFAULT',
      'reasonText': '默认需要确认',
      'policySource': 'auto_approval.dart',
      'scope': 'D:\\proj',
      'evidence': {'detail': 'Get-Date'},
      'reversible': false,
    });
    expect(entry.ts, '2026-11-15T05:12:33Z');
    expect(entry.reasonCode, 'ASK_DEFAULT');
    expect(entry.evidence['detail'], 'Get-Date');
    final payload = entry.toPayload();
    expect(payload.containsKey('ts'), isFalse, reason: 'ts 由宿主生成');
    expect(payload.containsKey('v'), isFalse);
    expect(payload['decision'], 'ask');
    expect(payload['scope'], 'D:\\proj');

    // 缺字段的老行也不炸（页面照样能显示）
    final sparse = SecurityEntry.fromJson(const {});
    expect(sparse.v, 1);
    expect(sparse.evidence, isEmpty);
    expect(sparse.reversible, isFalse);
  });
}
