# v1（Tauri + WebView2）文档冻结快照

> **冻结时间**：2026-11-03
> **对应版本**：`0.9.45`（`app/src-tauri/Cargo.toml`）
> **状态**：**只读**。这里的任何文件都**不再更新**；现行文档在 `docs/` 根目录。

## 这个目录是什么

v1 是 Lunac 的第一代前端：Tauri 2 + Windows WebView2，前端在 `app/src`，插件是
`app/src/plugins/builtin/*.ts`。迁移到 Flutter（v2）之前，把当时的文档**原样快照**一份，
用来回答三个问题：

1. 某个规则/约束**当初为什么这么定**（后来在 v2 里要不要保留）；
2. v1 的行为契约是什么（`stream-json` 事件、审批卡字段、工具 schema 语义）；
3. 迁移后若出现回归，拿哪一版对照。

## 快照内容

| 文件 | 当时的作用 |
|---|---|
| `README.md` | 文档索引（含旧文档去向） |
| `code-standards.md` / `forbidden.md` | 代码硬规则与禁用清单（v1 生效） |
| `testing.md` / `acceptance-map.md` / `acceptance-roadmap.md` | 测试清单与能力账本 |
| `todo.md` | 待办总账（含 2026-11-03 的代理真机结果） |
| `logging.md` | 日志规范（v1 落地口径） |
| `ui-style.md` / `theme-*.md` / `architecture-overview.html` | 视觉与架构说明 |
| `resource-hub-design.md` / `sandbox-research.md` / `cache-hit-experiment.md` | 专项设计 |
| `parse-minidump.ps1` | 崩溃分析脚本 |

## 使用纪律

- **不要修改本目录**（包括 typo）；要改规则请改 `docs/` 根目录的现行版本。
- v2 的规则如果与这里不同，**以 v2 文档为准**，并在
  [`../../flutter-migration.md`](../../flutter-migration.md) 的映射表里注明差异。
- 本目录内的相对链接指向的是**快照当时**的路径，可能与当前仓库不一致 —— 这是预期的。
