// ── 插件分类：基础插件 / 拓展插件（2026-09-29，用户定义）──────────────
//
// **基础插件 = 随安装包一起装、编译进 bundle、用户既不能装也不能卸。**
// 用户 2026-09-29 定的清单只有五个（含把「工具编辑器」并进 AI 助手）：
//
//   备忘录 / AI 助手（含工具编辑器）/ 网页搜索 / 设置 / 快速启动
//
// **翻译**是 2026-09-29 用户明确指定的第二个基础插件（第三条需求：「词典做底座 +
// 模型补漏 + 译文存数据库，作为基础插件」）—— 用户那条「之后新增的一切插件默认都是
// 拓展插件」的规矩仍然成立，翻译是**被点名**的那一个，不是默认。
//
// **拓展插件 = 其余全部。** 用户原话：「其他插件均列为拓展插件」，
// 且「之后的插件除非我说明都将为拓展插件处理」—— 所以这里的判据写成
// **白名单**：**不在 `BASE_PLUGIN_IDS` 里的一律按拓展插件**（含市场里装的、
// AI 自己写的、将来新增的）。将来要把某个插件提成基础插件，只能改这一张表。
//
// **卸载语义（用户 2026-09-29 的要求）**：拓展插件在「没装」的状态下必须
// **完全不存在于这个应用** —— 前端文件与依赖都不在、界面里也开不出来。
// 判据因此是「盘上有没有可用的一份」（`hasDiskPlugin()`），而**不是**「registry 里有没有」：
// bundle 里那份只对基础插件有意义。

/** 基础插件的 id 清单 —— **顺序就是面板上的展示顺序**（用户给的顺序，别随手改）。
 *  翻译排在最后：用户点名它为基础插件时没给位置，插在中间会打乱他定的那 5 个的顺序。 */
export const BASE_PLUGIN_IDS: readonly string[] = [
  "memo",            // 备忘录
  "ai-agent",        // AI 助手（含工具编辑器）
  "web-search",      // 网页搜索
  "settings",        // 设置
  "quick-launch",    // 快速启动
  "translate",       // 翻译（2026-09-29 点名加的基础插件）
];

const BASE = new Set<string>(BASE_PLUGIN_IDS);

/** 已并入别的插件的条目：仍是独立的 `Plugin`（可被搜索到、可被别的面板调起），
 *  但**不单独占市场表的一行** —— 否则用户会以为「工具编辑器」是要单独装/卸的东西，
 *  而用户 2026-09-29 定的是「AI 助手插件应该包括工具编辑器插件」。
 *  key = 被并进去的 id，value = 并到哪个 id 上。 */
export const MERGED_INTO: Readonly<Record<string, string>> = {
  "tool-editor": "ai-agent",
};

/** 是不是基础插件。白名单之外的**一律**是拓展插件。 */
export function isBasePlugin(id: string): boolean {
  return BASE.has(id);
}

/** 该不该在市场表里单独占一行（并进别的插件的不占）。 */
export function showsInMarket(id: string): boolean {
  return !(id in MERGED_INTO);
}

/** 基础插件的展示排序权重（不在清单里的给一个很大的值 —— 让拓展插件排后面）。 */
export function basePluginOrder(id: string): number {
  const i = BASE_PLUGIN_IDS.indexOf(id);
  return i < 0 ? Number.MAX_SAFE_INTEGER : i;
}
