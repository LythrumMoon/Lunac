// 白箱边界账本（W0，2026-11-01）：前端唯一出口。
//
// 设计口径见 docs/sandbox-research.md §6：
//   · 账本只记录「判定」，不执行任何拦截；真正的拦截仍在审批卡 / agent / 工作区锁；
//   · 原因码是稳定契约（与 core-agent 侧、MXC denials 的映射共用）；
//   · 写盘失败绝不打断对话（fire-and-forget，宿主只记一行 warn）。

import { invoke } from "@tauri-apps/api/core";
import { t } from "./i18n.js";

export type SecurityDecision = "allow" | "ask" | "deny" | "auto";

export type SecurityReasonCode =
  | "AUTO_READONLY"
  | "RUN_MODE_AUTO"
  | "WHITELIST_TOOL"
  | "WHITELIST_CMD"
  | "ASK_DEFAULT"
  | "DANGER_RULE"
  | "OPAQUE_COMMAND"
  | "SECRET_IN_PAYLOAD"
  | "PLAN_MODE_BLOCK"
  | "PLAN_APPROVAL"
  | "ASK_QUESTION"
  | "WORKSPACE_LOCK"
  | "PROTECTED_DIR"
  | "USER_ALLOW"
  | "USER_DENY"
  | "USER_ALWAYS"
  | "ENGINE_TIMEOUT"
  | "ENGINE_OOM"
  | "MXC_FILE_DENY"
  | "MXC_NET_DENY"
  | "MXC_CAP_DENY"
  | "MXC_UI_DENY"
  | "BACKEND_UNAVAILABLE";

export interface SecurityEntry {
  /** 行格式版本（宿主写 1）。 */
  v?: number;
  /** 宿主生成的 UTC 时间戳；前端传入时会被忽略。 */
  ts?: string;
  session?: string;
  actor: string;
  tool: string;
  target: string;
  decision: SecurityDecision;
  reasonCode: SecurityReasonCode;
  reasonText: string;
  policySource: string;
  scope: string;
  evidence?: Record<string, unknown>;
  reversible: boolean;
}

/** 写一条判定。**不 await** —— 账本是旁路，不能因为写盘慢拖住审批。 */
export function logSecurityDecision(entry: SecurityEntry): void {
  void invoke("security_ledger_append", { entry }).catch(() => {
    /* 宿主已记 warn；前端不打扰用户 */
  });
}

export async function recentSecurityDecisions(limit = 200): Promise<SecurityEntry[]> {
  return invoke<SecurityEntry[]>("security_ledger_recent", { limit });
}

export async function exportSecurityLedger(
  policy: Record<string, unknown>,
): Promise<string> {
  return invoke<string>("security_ledger_export", { policy });
}

/** 原因码 → 用户可读文案（唯一出口；别在调用点拼字符串）。
 *
 *  `detail` 只用于「命中哪条规则 / 哪个命令」，不做二次解释。 */
export function reasonTextFor(code: SecurityReasonCode, detail?: string): string {
  const d = (detail ?? "").trim();
  switch (code) {
    case "AUTO_READONLY":
      return t("security.reason.auto_readonly");
    case "RUN_MODE_AUTO":
      return t("security.reason.auto");
    case "WHITELIST_TOOL":
    case "WHITELIST_CMD":
      return d
        ? t("security.reason.whitelist_detail", { detail: d })
        : t("security.reason.whitelist");
    case "ASK_DEFAULT":
      return t("security.reason.ask_default");
    case "DANGER_RULE":
      return d
        ? t("security.reason.danger_detail", { detail: d })
        : t("security.reason.danger");
    case "OPAQUE_COMMAND":
      return d
        ? t("security.reason.opaque_detail", { detail: d })
        : t("security.reason.opaque");
    case "SECRET_IN_PAYLOAD":
      return d
        ? t("security.reason.secrets_detail", { detail: d })
        : t("security.reason.secrets");
    case "PLAN_APPROVAL":
      return t("security.reason.plan");
    case "ASK_QUESTION":
      return t("security.reason.question");
    case "USER_ALLOW":
      return t("security.reason.user_allow");
    case "USER_DENY":
      return t("security.reason.user_deny");
    case "USER_ALWAYS":
      return t("security.reason.user_always");
    default:
      return t("security.reason.other");
  }
}
