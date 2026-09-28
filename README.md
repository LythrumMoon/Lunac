# Lunac

Windows 桌面启动器 / 搜索工具。按全局热键唤出搜索栏，搜本机应用、内置插件与 AI 助手，回车直达。

基于 **Tauri 2 + WebView2** 构建，常驻系统托盘，窗口高度由内容实测驱动。

---

## 功能特性

| 能力 | 说明 |
|------|------|
| 全局热键 | 默认 `Ctrl+Alt+Space`；优先内核级 `RegisterHotKey`（不装键盘钩子），仅在无法注册时回退低级键盘钩子。可在设置面板录制改键 |
| 应用搜索 | 扫描开始菜单（带 TTL 缓存 + 落盘缓存 + 后台预刷新），中文支持全拼 / 首字母模糊匹配 |
| AI 助手 | 内置 Agent 后端（stream-json 流式），支持工具调用、权限审批卡片、token 计费仪表盘 |
| 插件市场 | 从插件市场按需下载插件，**依赖随插件一起装**；装完立即生效，不必重启 AI |
| 插件悬浮窗 | 插件可脱离主窗口独立成窗（置顶 / 最小化），与搜索栏同时存在 |
| 多语言 | 跟随 Windows 系统语言（zh-CN / zh-TW / ja / ko / en） |
| VS Code 扩展 | 把 AI 对话搬进 VS Code 侧边栏（见 `vscode-extension/`） |

## 内置插件

| 插件 | 触发关键词 |
|------|-----------|
| 快速启动 Quick Launch | open / launch / run / app / 打开 / 启动 / 运行 |
| 设置 Settings | settings / shortcut / hotkey / config / 设置 / 快捷键 |
| 剪贴板历史 Clipboard History | clipboard / history / paste / 剪贴板 / 粘贴 |
| 网页搜索 Web Search | search / google / baidu / bing / 搜索 / 网页 |
| 工具编辑器 Tool Editor | tool / tools / 工具 / mcp / agent / 技能 |
| 文字识别 OCR | ocr / 识别 / 文字识别 / 图片转文字 |
| 备忘录 Memo | memo / note / 备忘录 / 笔记 |
| 音乐 Music | music / 歌词 / spotify / 正在播放 |
| 文件转换 Convert | convert / 转换 / 转码 / ffmpeg |
| AI 助手 AI Agent | 无匹配时的兜底入口 |

## 用户启动环境要求

| 依赖 | 要求 | 说明 |
|------|------|------|
| Windows | 10 / 11（x64） | 仅支持 Windows |
| WebView2 Runtime | 任意版本 | Win11 自带；Win10 若缺失由安装包自动补装 |

安装包为单文件 NSIS（当前用户级安装，不需要管理员权限）。所有缓存与业务数据落在 **exe 安装根目录**下，卸载时一并清除。

## 开发者修改环境要求

| 依赖 | 版本 | 用途 |
|------|------|------|
| Rust | stable | 编译宿主 `lunac.exe` 与 Agent 后端 `agent.exe` |
| Node.js | ≥ 18 | 前端构建（TypeScript + Vite） |
| NSIS | 3.x | 打安装包（仅发布时需要；`makensis` 需在 PATH 或默认安装路径） |
| 7-Zip | 任意 | 可选，仅用于发布脚本校验包内容 |

常用命令：

```powershell
powershell -ExecutionPolicy Bypass -File scripts\dev.ps1            # 开发模式（前端 dev server + Tauri）
powershell -ExecutionPolicy Bypass -File build-release.ps1          # 打完整安装包（版本自动 +1）
powershell -ExecutionPolicy Bypass -File scripts\build-plugins.ps1  # 打包可独立分发的插件（见下）
```

插件的目录约定、清单字段与入口契约见 [Modules 插件开发规范](./agent-templates/modules/README.md)（安装后位于 `<exe 根>\Modules\README.md`）。

## 第三方组件与许可

本项目以 MIT 许可开源（见 [LICENSE](./LICENSE)）。使用到的第三方组件：

| 组件 | 许可 / 说明 |
|------|------|
| [Tauri 2](https://tauri.app/) | MIT / Apache-2.0 |
| [PaddleOCR-json](https://github.com/hiroi-sora/PaddleOCR-json) | 离线 OCR 引擎与 PP-OCRv4 模型，**单独下载，不随仓库分发** |
| [pinyin-pro](https://github.com/zh-lx/pinyin-pro) | MIT，中文拼音 / 首字母匹配 |
| [librespot](https://github.com/librespot-org/librespot) | MIT，Spotify 音频播放（运行时可选用，不随仓库分发） |
| [LRCLIB](https://lrclib.net/) | 歌词数据来源（公开 API） |
| WebView2 Runtime | Microsoft 分发，随系统或由安装包补装 |

`docs/` 下的部分方法论章节改编自 [Hermes Agent](https://github.com/obra/superpowers)（MIT）。
