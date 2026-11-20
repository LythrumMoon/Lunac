// 待办依赖的前端判定（§待办 3-22 分片 2，2026-10-31）。
//
// 为什么单独成一个模块：抽屉（`main.ts` 的 `renderTodoDrawer`）只该管「怎么画」，
// 「哪条能开始、哪条在等前置」是**可单测的纯逻辑** —— 放进 DOM 渲染里就只能靠肉眼看。
// 口径与后端 `core-agent/src/tools.rs` 的 `todo_write` **一致**（那边拦的是「标 in_progress
// 但前置未完成」，这边只是把同一套数据画出来），前端不新增 IPC、也不维护第二份状态：
// 清单的唯一真相仍是模型最近一条 `TodoWrite` 的 `tool_use` 参数。

export type TodoStatus = "pending" | "in_progress" | "completed";

export interface TodoDecor {
  status: TodoStatus;
  /** 未完成、且有前置没完成 ⇒ 现在不能开工。 */
  blocked: boolean;
  /** 还是 pending、有前置且前置全完成 ⇒ 是「接下来该做」的那条。 */
  ready: boolean;
  /** 尚未完成的前置 id（用于 tooltip；未知 id 也按未完成算 —— 宁可多等一步）。 */
  blockers: string[];
}

/**
 * 把一份 `TodoWrite` 的 `todos` 数组换算成每条的展示态。
 *
 * 容错口径与渲染层一致：`status` 只认三个规范名，其余按 `pending`；`id` / `blocked_by`
 * 缺失或类型不对就忽略（后端已做过严格校验，这里只负责不崩）。
 */
export function decorateTodos(items: unknown[]): TodoDecor[] {
  const rows = items.map((raw) => {
    const o = (raw ?? {}) as Record<string, unknown>;
    const status: TodoStatus =
      o.status === "completed" || o.status === "in_progress" ? o.status : "pending";
    const id = typeof o.id === "string" && o.id.trim() !== "" ? o.id.trim() : undefined;
    const blockedBy = Array.isArray(o.blocked_by)
      ? o.blocked_by
          .filter((v): v is string => typeof v === "string" && v.trim() !== "")
          .map((v) => v.trim())
      : [];
    return { status, id, blockedBy };
  });

  const completed = new Map<string, boolean>();
  for (const row of rows) {
    if (row.id) completed.set(row.id, row.status === "completed");
  }

  return rows.map((row) => {
    const blockers = row.blockedBy.filter((id) => completed.get(id) !== true);
    return {
      status: row.status,
      blocked: row.status !== "completed" && blockers.length > 0,
      ready: row.status === "pending" && row.blockedBy.length > 0 && blockers.length === 0,
      blockers,
    };
  });
}
