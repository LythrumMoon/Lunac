# Lunac

uTools 风格的 Windows 桌面启动器 / 搜索工具。按全局热键唤出一个毛玻璃搜索栏，搜索本机应用、插件与 AI 助手，回车直达。

基于 **Tauri 2 + WebView2** 构建，常驻系统托盘，窗口高度由内容实测驱动。

---

## 功能特性

| 能力 | 说明 |
|------|------|
| 全局热键 | 默认 `Ctrl+Alt+Space`；优先内核级 `RegisterHotKey`（不装键盘钩子），仅在无法注册时回退低级键盘钩子。可在设置面板录制改键 |
| 应用搜索 | 扫描开始菜单（带 TTL 缓存 + 落盘缓存 + 后台预刷新），中文支持全拼 / 首字母模糊匹配 |
| AI 助手 | 内置 Agent 后端（stream-json 流式），支持工具调用、权限审批卡片、token 计费仪表盘 |
| 离线 OCR | PaddleOCR-json（PP-OCRv4 模型），中英文图片转文字，无需联网 |
| 备忘录 | 本地保存，支持粘贴/拖放图片、自定义检索标识，搜索栏直达 |
| 剪贴板历史 | 自动记录复制的文本与文件路径，一键回填 |
| 自定义启动 | 拖入 / 粘贴任意文件路径即注册为启动项，面板可管理 |
| VS Code 扩展 | 把 AI 对话搬进 VS Code 侧边栏（见 `vscode-extension/`） |

### 内置插件（8 个）

| 插件 | 触发关键词 |
|------|-----------|
| 🚀 快速启动 Quick Launch | open / launch / run / app / 打开 / 启动 / 运行 |
| ⚙️ 设置 Settings | settings / shortcut / hotkey / config / 设置 / 快捷键 |
| 📋 剪贴板历史 Clipboard History | clipboard / history / paste / 剪贴板 / 粘贴 |
| 🌐 网页搜索 Web Search | search / google / baidu / bing / 搜索 / 网页 |
| 🔧 工具编辑器 Tool Editor | tool / tools / 工具 / mcp / agent / 技能 |
| 🔍 文字识别 OCR | ocr / 识别 / 文字识别 / 图片转文字 |
| 📝 备忘录 Memo | memo / note / 备忘录 / 笔记 |
| 🤖 AI 助手 AI Agent | 无匹配时的兜底入口 |

---

## 技术栈

- **前端**：TypeScript + Vite + 原生 DOM（无框架），毛玻璃主题
- **后端**：Rust（Tauri 2），原生 Win32 互操作（热键 / 剪贴板 / 窗口管理）
- **渲染**：WebView2（透明无边框窗口）
- **AI 引擎**：内置 Agent 后端（stream-json 流式协议）
- **国际化**：跟随 Windows 系统语言（zh-CN / zh-TW / ja / ko / en）

---

## 环境要求

| 依赖 | 版本 | 用途 |
|------|------|------|
| Windows | 10 / 11 | 仅支持 Windows |
| Rust | stable | 编译后端 |
| Node.js | ≥ 18 | 前端构建 |
| WebView2 Runtime | 任意 | 系统自带（Win11 默认） |

---

## 配置

所有 AI 相关配置通过 `app/src-tauri/.env` 提供，Rust 侧读取顺序（见 `commands.rs`）：

| 项 | 环境变量 |
|----|---------|
| 供应商 | `AI_PROVIDER` |
| 接口地址 | `AI_API_URL` → `DEEPSEEK_URL` |
| 密钥 | `AI_API_KEY` → `DEEPSEEK_API_KEY` |
| 模型 | `AI_MODEL` |

> 接口地址请填写不带 `/v1` 后缀的基础地址。

---

## 数据目录

所有缓存与业务数据统一放在 **exe 安装根目录**下（而非 `%LOCALAPPDATA%`），dev 与 release 数据因此天然隔离：

```
<exe 所在目录>\
├── ModuleData\
│   ├── history\       聊天会话、剪贴板历史
│   ├── memo\          备忘录（含 images\<id>\ 图片）
│   └── custom\        自定义启动项注册表
├── temp\
│   ├── webview-data\  WebView2 用户数据
│   └── app-index-cache.json  应用扫描缓存
├── skills\            用户技能（SKILL.md）
├── tools\             自定义 Agent Tools
├── config\hotkey.json 热键配置
└── paddle-ocr\        离线 OCR 引擎与模型
```

卸载时由 NSIS 钩子做双清理：安装目录内的上述运行时目录 + 旧版本遗留的 `%LOCALAPPDATA%\Lunac(-dev)`。

---

## 已知问题

**Windows Defender 误报**

未签名的 `lunac.exe` 可能被 Defender 云端机器学习判定为 `Trojan:Win32/Prowloc.A!cl`（`!cl` 后缀 = 云端 ML 启发式，非签名匹配）。这是误报，常见诱因是全局热键钩子等系统级行为。

- 临时：Windows 安全中心 → 病毒和威胁防护 → 排除项，添加安装目录
- 根治：向 [Microsoft 提交误报](https://www.microsoft.com/en-us/wdsi/filesubmission)（选 *Software developer* → *Incorrectly detected as malware*），或为构建产物做代码签名

---

## 第三方组件与许可

本项目以 MIT 许可开源（见 [LICENSE](./LICENSE)）。使用到的第三方组件：

| 组件 | 说明 |
|------|------|
| [Tauri 2](https://tauri.app/) | MIT / Apache-2.0 |
| [PaddleOCR-json](https://github.com/hiroi-sora/PaddleOCR-json) | 离线 OCR 引擎（单独下载，不随仓库分发） |

`docs/` 下的部分方法论章节改编自 [Hermes Agent](https://github.com/obra/superpowers)（MIT）。

---

## 文档

- [docs/ai-spec.md](./docs/ai-spec.md) — 架构与既定规则
- [docs/code-rules.md](./docs/code-rules.md) — 代码规则与反模式速查（改动前必读）
- [docs/icon-style.md](./docs/icon-style.md) — 图标风格规范
