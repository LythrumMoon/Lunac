# Lunac 工具目录（tools）

把你自己的工具定义（JSON）放进这个目录，agent 启动时会加载它们，并以 `mcp__<名字>` 的形式暴露给模型，**由模型自行决定何时调用**；调用前一定会先弹审批卡，等你在界面上点「允许」。

> 与技能（`skills\`）的区别：技能是**给模型看的说明文档**（教它怎么做），工具是**真正会被执行的动作**（shell / HTTP / 内置能力），所以每个工具都必须先过审批。

## ⚠️ 先分清「该放哪」（2026-10-07）

本目录**只放 MCP 工具定义**（`*.json`）。其它三类东西各有各的家，别混：

| 你要做的东西 | 放哪 |
|---|---|
| **Lunac 插件**（搜索框里加载的 UI 模块） | `<exe 根>\Modules\<id>\` |
| **AI 技能**（给模型的说明文档） | `<exe 根>\skills\<技能名>\SKILL.md` |
| **MCP 工具**（`mcp__*`，本目录的 `.json`） | **本目录** |
| **其它一切**（独立工具 / 脚本 / 工程，例如 Photoshop CEP 面板、Python 生成器、自用 CLI） | **工作目录**（agent 的 cwd，默认 `<exe 根>\temp\transStorage`）下自建文件夹 |

判据一句话：**既不被 Lunac 的搜索面板加载、也不是「给模型的说明文档」或「给模型的工具定义」，就不该放进 skills / Modules / tools 这三者中的任何一个。**

## 目录结构

```
tools\
  my-tool.json        ← 每个 .json 就是一个工具；文件名随意
```

## JSON 格式

```json
{
  "name": "tool_name",
  "description": "一句话说明这个工具做什么、什么时候该用",
  "inputSchema": {
    "type": "object",
    "properties": {
      "param1": { "type": "string", "description": "参数说明" }
    },
    "required": ["param1"]
  },
  "handler": { "type": "shell", "command": "powershell -NoProfile -Command \"...{{param1}}...\"" }
}
```

| 字段 | 说明 |
|---|---|
| `name` | 工具名，进请求体后会被加上 `mcp__` 前缀；只保留字母 / 数字 / `-` / `_`，其余字符换成 `_`，整体超 64 字符截断（重名会再补 `_2`） |
| `description` | **必填且要短** —— 每一轮请求都会带上，直接影响上下文体积与缓存 |
| `inputSchema` | 标准 JSON Schema；`required` 里的参数模型必须给 |
| `handler.type` | `shell`（执行命令）/ `http`（发请求）/ `builtin`（Lunac 进程内实现，见下） |
| `elicit` | 可选。声明 `{ message, requestedSchema }` 后，**调用该工具前会先向用户弹卡**收集补充输入，拿到答案并入参数再执行 handler（与 `inputSchema` 是两回事：一个是给模型的契约，一个是给用户填的表单） |

### handler 三种形态

```json
{ "type": "shell", "command": "..." }
{ "type": "http",  "method": "GET", "url": "https://...", "headers": {}, "body": "..." }
{ "type": "builtin", "name": "image_pattern_analysis" }
```

参数占位符写作 `{{参数名}}`（也兼容 `{{ 参数名 }}`），执行前会被替换成模型给的值。

## 生效与安全

- 新增 / 修改 / 删除工具后需要**重启 agent** 才生效（工具编辑器面板会在保存后自动重启）。
- 工具清单**按名字排序**后才进请求体 —— 顺序抖动会让端点侧前缀缓存整段失效。
- 只读档（`plan`）下 MCP 工具**不接入**（接进来只会每次被拒，还会让工具清单随档位漂移）。
- 工具输出的单次结果超过 **12000 字符**时，会把**全文**落到 `<exe 根>\temp\tool-outputs\`，上下文里只内联
  「头 8000 + 尾 2000 + 文件路径」；模型要全文自己用 `Read` / `Grep` 去取（不是旧版那种「硬截断、超出即丢」）。
- 报错以 `isError` 回给模型，不会中断整轮对话。

## 关于本目录里的示例

`example-tool.json.example` 是一份可以照抄的模板。它**故意不叫 `.json`**，所以不会被加载 —— 想启用就把它改名成 `example-tool.json`。
