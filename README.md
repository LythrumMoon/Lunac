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

## 插件（第三方扩展）

除上表的内置插件外，其余能力（OCR、音乐、桌宠、文件转换、剪贴板历史、代理、本地决策模型…）
以**插件**形式按需安装：打开 **设置 → 插件** 从插件市场下载即可（依赖随插件一起装，装完立即生效）。

| 项目 | 说明 |
|------|------|
| 分发仓库 | [`LythrumMoon/lunac-plugins`](https://github.com/LythrumMoon/lunac-plugins)（公开，无需鉴权） |
| 索引 | `index.json`（宿主常量 `plugin_market::INDEX_URL` 指向它）；索引只在**打开设置面板时**由宿主现拉，另受 GitHub raw 的 CDN 缓存影响（改完索引约几分钟内可见） |
| 插件包 | `packages/<分类>/<id>-<版本>.zip`（下表为分类约定；索引里存**整条** url，故子目录不影响拉取） |
| 开发规范 | [Modules 插件开发规范](./agent-templates/modules/README.md)（目录约定、清单字段、依赖声明、入口契约） |
| 发布链 | `scripts\build-plugins.ps1`（打包）→ `scripts\publish-plugins.ps1`（分发 + 更新索引），每个插件**只保留最高版本** |

分类（即插件仓库 `packages\` 下的目录名）：

| 分类 | 插件 |
|------|------|
| `productivity` | 文件转换 convert、剪贴板历史 clipboard-history |
| `media` | 音乐 music |
| `ocr` | 文字识别 ocr |
| `pet` | 桌宠 pet |
| `ai` | 本地决策模型 local-model |
| `system` | 代理 proxy |

**大资产单独分发**：桌宠的 Live2D Cubism Core（专有）与语音包（ZipVoice）体积大（数十 MB ~ 数百 MB），
**不随本仓库或插件 zip 进 git** —— 它们是插件 `lunac-plugin.json` 里的 `dependencies`，安装时由宿主
从插件仓库的 **Release** 资产下载并按 `sha256` 校验。厂商权属与**非商用限制**见下节。

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
| [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) | Apache-2.0，桌宠语音（ZipVoice）运行库，随插件 `archive` 依赖下载 |
| [ONNX Runtime](https://github.com/microsoft/onnxruntime) | MIT，随桌宠语音资产分发（`onnxruntime.dll`） |
| [ZipVoice](https://github.com/k2-fsa/ZipVoice) | 预训练权重为 **CC BY-NC 4.0（仅限非商用）**，随桌宠语音资产分发，**不得商用** |
| [eSpeak NG](https://github.com/espeak-ng/espeak-ng) 数据 | GPL-3.0（copyleft），随桌宠语音资产分发；对应源码见上游项目 |
| [Vocos](https://huggingface.co/charactr/vocos-mel-24khz) | MIT，桌宠语音的声码器权重（`vocos_24khz.onnx`），随语音资产分发 |
| [Live2D Cubism Core](https://www.live2d.com/en/sdk/download/web/) | 专有许可，桌宠 Live2D 引擎运行时，随插件 `file` 依赖下载（随附官方 LICENSE） |
| [pinyin-pro](https://github.com/zh-lx/pinyin-pro) | MIT，中文拼音 / 首字母匹配 |
| [librespot](https://github.com/librespot-org/librespot) | MIT，Spotify 音频播放（运行时可选用，不随仓库分发） |
| [LRCLIB](https://lrclib.net/) | 歌词数据来源（公开 API） |
| WebView2 Runtime | Microsoft 分发，随系统或由安装包补装 |

**本项目为个人项目，明确非商用。** 随桌宠语音资产下载的 ZipVoice 预训练权重为 **CC BY-NC 4.0**，
**仅限非商业用途**：任何商业使用均需自行向权利方（k2-fsa / Next-gen Kaldi）取得授权，本项目的 MIT
许可**不**涵盖该权重。`espeak-ng-data` 为 **GPL-3.0**，其对应源码可从上游项目获取。

本项目的 MIT 许可（[LICENSE](./LICENSE)）仅覆盖本项目原创代码；上表所有第三方组件
各自适用其自身许可**，不受 MIT 覆盖。下游使用者须自行遵守相应许可。

各组件许可**全文**位于 [`licenses/`](./licenses) 目录：

- [`licenses/zipvoice/`](./licenses/zipvoice)：ZipVoice 语音组件 —— CC BY-NC 4.0 / Apache-2.0 / MIT /
  GPL-3.0 / Vocos MIT 全文，及 `THIRD_PARTY_NOTICES.md`（组件 × 授权 × 来源 × 义务 + 非商用声明）。
- [`licenses/live2d-cubism-core/`](./licenses/live2d-cubism-core)：Live2D Cubism Core 的官方 LICENSE 与
  可再分发文件清单。

**下游照做**：若你转分发本项目的安装包、插件包或上述大资产，必须连同对应的授权文件一起携带
（例如 Live2D 的 `LICENSE.md` / `RedistributableFiles.txt`、ZipVoice 资产内的 `LICENSES/` 与
`THIRD_PARTY_NOTICES.md`）。插件仓库 [`LythrumMoon/lunac-plugins`](https://github.com/LythrumMoon/lunac-plugins)
另行分发的某个插件若携带 / 依赖上表组件，其随附义务同样适用，该仓库 README 有对应说明。

`docs/` 下的部分方法论章节改编自 [Hermes Agent](https://github.com/obra/superpowers)（MIT）。
