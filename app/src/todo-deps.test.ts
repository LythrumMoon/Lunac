import { describe, expect, it } from "vitest";
import { decorateTodos } from "./todo-deps";

describe("decorateTodos", () => {
  it("splits pending entries into ready and blocked", () => {
    const rows = decorateTodos([
      { id: "a", content: "build", status: "completed" },
      { id: "b", content: "test", status: "pending", blocked_by: ["a"] },
      { id: "c", content: "ship", status: "pending", blocked_by: ["b"] },
    ]);
    expect(rows[1]).toMatchObject({ ready: true, blocked: false, blockers: [] });
    expect(rows[2]).toMatchObject({ ready: false, blocked: true, blockers: ["b"] });
  });

  it("never marks a completed or in_progress entry as ready", () => {
    const rows = decorateTodos([
      { id: "a", content: "build", status: "completed" },
      { id: "b", content: "test", status: "in_progress", blocked_by: ["a"] },
      { id: "c", content: "old", status: "completed", blocked_by: ["b"] },
    ]);
    expect(rows[1].ready).toBe(false);
    expect(rows[1].blocked).toBe(false);
    expect(rows[2].blocked).toBe(false);
  });

  it("treats unknown blockers as unfinished (safe side)", () => {
    const rows = decorateTodos([
      { id: "b", content: "test", status: "pending", blocked_by: ["ghost"] },
    ]);
    expect(rows[0].blocked).toBe(true);
    expect(rows[0].blockers).toEqual(["ghost"]);
  });

  it("tolerates malformed input without throwing", () => {
    const rows = decorateTodos([
      null,
      { status: "weird", blocked_by: "not-an-array" },
      { id: "  ", blocked_by: [1, "", "ok"] },
    ]);
    expect(rows.map((r) => r.status)).toEqual(["pending", "pending", "pending"]);
    expect(rows[2].blocked).toBe(true);
    expect(rows[2].blockers).toEqual(["ok"]);
  });
});
