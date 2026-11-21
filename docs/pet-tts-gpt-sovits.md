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

## 安装 gsv.cpp

1. 下载 Windows 一键包（含引擎、嵌入式 Python、模型、GAG GUI）：
   <https://huggingface.co/KakaruHayate/GSV.CPP> → `windows/GSV.CPP-win64-*.zip`
   （镜像：`https://hf-mirror.com/KakaruHayate/GSV.CPP`）。
2. 解压到任意目录（不要放进 `Modules/pet`，模型较大，单独管理）。
3. 运行 `Start GAG.bat`，确认服务监听 `127.0.0.1:9880`。
   - 也可以在解压目录手动启动 `gsv_server --models models --device vulkan --port 9880`
     和文本前端 sidecar（见 gsv.cpp 的 `docs/build.md`）。
   - 没有 Vulkan 显卡时用 `--device cpu`，但实时性会明显下降。

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
