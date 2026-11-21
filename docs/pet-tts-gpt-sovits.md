# 桌宠 TTS：GPT-SoVITS V5（gsv.cpp）

> 2026-11-21 用户裁定：桌宠语音采用 **GPT-SoVITS V5**，不再以 ZipVoice 为主引擎。
> v2 的 `config/pet-voice.json` 默认 `engine = gpt_sovits`；旧配置仍可按 `engine = zipvoice` 读回。

## 为什么选它

| 维度 | GPT-SoVITS V5 |
|---|---|
| 许可 | MIT（代码 + 权重；gsv.cpp 也是 MIT） |
| 中文 | 中文社区长期验证，V5 提升相似度与稳定性 |
| 参考音色 | 5 秒零样本克隆 |
| 训练音色 | 1 分钟数据可微调 |
| Windows | 有整合包；gsv.cpp 支持 Vulkan/CPU，不依赖 CUDA |
| 插件接入 | 自带 `gsv_server` HTTP `/tts`，可直接作为桌宠 sidecar |
| 速度 | RTX 2070 / Vulkan：3.4 秒音频整条链路约 945ms；CPU 明显慢，只适合离线/预生成 |

其他候选（Ming-3.0-tts-0.5B、Fun-CosyVoice3-0.5B、Qwen3-TTS-0.6B）中文指标也很强，
但都需要 PyTorch/CUDA 运行时，打包与维护成本高于 gsv.cpp；本项目先落地 GPT-SoVITS。

## 模型不分发，由用户下载

Lunac 安装包**不包含** gsv.cpp / 模型。桌宠控制台「语音朗读」的**资产目录**一行提供：

- **GitHub 下载**：打开 `githubUrl`（默认 `LythrumMoon/lunac-plugins` 的 Releases；可把自制的运行包上传到那里）
- **HuggingFace 下载**：打开 `hfUrl`（默认 gsv.cpp 官方模型仓库）
- **选择目录**：选择整个 gsv.cpp 运行包目录（不是单个模型文件；gsv 需要同目录的 `models/`、文本前端、`Start` 脚本等）
- **启动服务**：按目录里的脚本 / `gsv_server.exe` 启动，默认 Vulkan 独显

默认资产目录：`<exe 根>/Modules/pet/gsv`。
把下载的 Windows 包解压到该目录（或任意目录后用「选择目录」指过去）。
目录里需要能找到 `Start*.bat` / `gsv_server.exe` 和至少一个 `.gguf` 模型；
状态行会显示识别到的模型文件数。

## 自动启动

- `config/pet-voice.json` 的 `autoStart` 默认 `true`。
- 打开桌宠插件（控制台 / 形象窗）时，宿主会检查 `serverUrl`；没连通就按资产目录启动服务。
- 换模型目录时先停掉本进程拉起的旧服务，再用新目录重启。
- 外部已启动的服务不会被重复拉起，也不会被停掉。

## 在 Lunac 桌宠控制台配置

路径：搜索「桌宠」→ 控制台 → **语音朗读**。

1. 引擎选 **GPT-SoVITS**。
2. 服务填 `http://127.0.0.1:9880`（gsv.cpp 默认）。
3. 采样步数：`v5turbo` 用 `4`；`v5dev` 用 `32`。
4. 点「选择音频」导入 **3~10 秒**参考录音（wav/mp3/flac 等），再填写这段录音里说的那句话。
5. 保存 → 试听。服务没连通、参考音频缺失、参考文本为空时，面板会直接给出原因。

配置落点：`<exe 根>/config/pet-voice.json`：

```json
{
  "enabled": true,
  "engine": "gpt_sovits",
  "serverUrl": "http://127.0.0.1:9880",
  "sampleSteps": 4,
  "textLang": "all_zh",
  "modelDir": "D:/Lunac/Modules/pet/gsv",
  "device": "vulkan",
  "autoStart": true,
  "githubUrl": "https://github.com/LythrumMoon/lunac-plugins/releases",
  "hfUrl": "https://huggingface.co/KakaruHayate/GSV.CPP",
  "referenceAudio": "D:/voices/me.wav",
  "referenceText": "这是参考录音里说的那句话。",
  "speed": 1.0,
  "volume": 1.0
}
```

`textLang`：`all_zh`（中文）/ `all_en`（英文）/ `auto`（自动）。

## 训练专属音色（可选）

1. 在 GPT-SoVITS WebUI 里用 1 分钟以上干净录音做微调（v5）。
2. 用 gsv.cpp 的 `tools/export_gguf` 把 checkpoint 转成 GGUF。
3. 按 gsv.cpp 的 `models/voices/<name>/` 布局放入参考音频与 manifest，重启服务。
4. Lunac 侧只换 `referenceAudio` / `referenceText`，不需要重新打包插件。

## 运行链路

```text
ChatPanel 回合结束（桌宠窗开着）
  └─ lunac_pet_voice_speak(text)
       └─ Rust: POST http://127.0.0.1:9880/tts
            { text, text_lang, ref_audio_path, prompt_text, prompt_lang, sample_steps }
       └─ 收到 WAV → temp/pet-tts/*.wav → rodio 播放
```

- 只朗读本轮最后一段助手正文，最多 500 字；思考 / 工具输出不朗读。
- 桌宠窗没开、语音关着、服务未启动时静默跳过（预览按钮会显示具体原因）。

## 许可与分发

- GPT-SoVITS、gsv.cpp：MIT。
- gsv.cpp 的文本前端/模型依赖有各自的 THIRD_PARTY_NOTICES，分发时随包保留。
- 参考音频必须是用户自己的声音或已获得授权的声音；插件只在本机使用，不上传。
