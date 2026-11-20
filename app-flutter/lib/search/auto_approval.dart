// 命令审批的「要不要问」判定（v1 `main.ts` 里 permission decision 的等价物）。
//
// 只做这一个决定，不碰协议、不碰 UI —— 单测可以直接喂审批请求里的字段。
//
// 口径（与 v1 保持一致）：
//   · agent 静态分析命中的危险命令（`analysis.dangerous`）/ 凭据（`analysis.secrets`）
//     ⇒ **任何档位都要人看**（含「自动」档）；
//   · `auto`      ⇒ 其余全部放行（含「判不出来」的 opaque 命令）；
//   · `allowlist` ⇒ 只放行 agent 判为「可证只读」的命令；v2 暂无用户白名单，
//                    其余一律询问；
//   · `manual`    ⇒ 全部询问；
//   · `AskUserQuestion` / `ExitPlanMode` ⇒ 永远询问：自动放行等于替用户回答问题 /
//     替用户批准计划，正是这两张卡要防的事；
//   · `Screenshot` ⇒ 永远询问（§待办 3-28.5）：抓的是**用户屏幕**，可能带出聊天窗口 /
//     密码框 / 其他应用内容 —— 隐私不该被「自动档」静默放行（与「危险命令永远人看」同一条纪律）。
//   · `RunScript` ⇒ 永远询问（§待办 3-28.15）：它跑的是**模型写的代码**。脚本内部只有只读
//     绑定，但「批准这段程序」是一次独立裁决 —— 自动放行等于把唯一裁决点也省掉
//     （docs/forbidden.md §禁用 1 第 4 条：批了脚本 ≠ 批了它做的一切）。
bool shouldAutoApprove({
  required String toolName,
  required Map<String, dynamic>? analysis,
  required String runMode,
}) {
  final dangerous = analysis?['dangerous'];
  if (dangerous is List && dangerous.isNotEmpty) {
    return false;
  }
  final secrets = analysis?['secrets'];
  if (secrets is List && secrets.isNotEmpty) {
    return false;
  }
  if (toolName == 'AskUserQuestion' ||
      toolName == 'ExitPlanMode' ||
      toolName == 'Screenshot' ||
      toolName == 'RunScript') {
    return false;
  }
  switch (runMode) {
    case 'auto':
      return true;
    case 'manual':
      return false;
    default:
      // allowlist：只认 agent 的「可证只读」结论（缺 analysis 一律不放行）。
      if (toolName == 'Cmd' || toolName == 'PowerShell') {
        return analysis?['readonly'] == true;
      }
      return false;
  }
}
