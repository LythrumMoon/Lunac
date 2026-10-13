# ZipVoice 语音组件 · 第三方组件与授权声明

本目录（`zipvoice/`）是 Lunac 桌宠的语音合成（TTS）运行库与模型。它由宿主的插件安装在
桌宠插件的 `lunac-plugin.json` 里按 `dependencies`（`type: "archive"`）下载、解压到此目录，
宿主只读不写（布局契约见 `app/src-tauri/src/tts_zipvoice.rs` 的 `zipvoice_dir()` /
`assets_ready()`）。请在分发、转发或再使用前读完本声明。

> **本项目（Lunac）为个人项目，明确非商用，仅在 GitHub 公开分发。**
> 随本组件分发的 ZipVoice 预训练权重为 **CC BY-NC 4.0**（署名—非商业性使用），
> **仅限非商业用途**。**任何商业使用都必须自行向权利方取得授权**，本项目的许可不覆盖它。

## 一、组件 × 授权 × 来源 × 义务

| 组件（本目录内文件） | 授权 | 来源 | 主要义务 |
|---|---|---|---|
| sherpa-onnx v1.13.8：`sherpa-onnx-c-api.dll`（及同目录 `onnxruntime.dll` 由它加载） | Apache-2.0 | k2-fsa/sherpa-onnx（Next-gen Kaldi） | 保留版权与许可声明；附 `LICENSES/Apache-2.0.txt` |
| ONNX Runtime：`onnxruntime.dll`、`onnxruntime_providers_shared.dll` | MIT | microsoft/onnxruntime | 保留版权与许可声明；附 `LICENSES/ONNX-Runtime-MIT.txt` |
| ZipVoice 预训练权重（ZipVoice-Distill）：`model/encoder.int8.onnx`、`model/decoder.int8.onnx`、`model/tokens.txt`、`model/lexicon.txt` | **CC BY-NC 4.0（非商用）** | k2-fsa/ZipVoice 模型（HF `k2-fsa/ZipVoice`），经 k2-fsa/sherpa-onnx 导出为 ONNX（int8） | 署名 + 附许可全文 + 标注改动 + **不得用于商业目的**；附 `LICENSES/CC-BY-NC-4.0.txt` |
| espeak-ng 数据：`model/espeak-ng-data/`（`lang/`、`voices/`、`*_dict`、`phondata` 等） | GPL-3.0（copyleft） | espeak-ng 项目（eSpeak NG） | 附 GPL-3.0 全文 + 提供对应源码获取途径；附 `LICENSES/GPL-3.0.txt` |
| Vocos 声码器权重：`vocos_24khz.onnx` | MIT | charactr/vocos-mel-24khz（代码库 gemelo-ai/vocos，原 charactr/vocos） | 保留版权与许可声明；附 `LICENSES/Vocos-MIT.txt` |

各授权全文见同目录 `LICENSES/`：
`Apache-2.0.txt`、`ONNX-Runtime-MIT.txt`、`CC-BY-NC-4.0.txt`、`GPL-3.0.txt`、`Vocos-MIT.txt`。

## 二、非商业性使用声明（CC BY-NC 4.0）

ZipVoice 的**预训练权重**（上表第 3 行；即随本组件分发的 `model/encoder.int8.onnx`、
`model/decoder.int8.onnx` 等）由上游以下原话明示为 **CC BY-NC**：

> "Our code is released under the Apache 2.0 License. The pre-trained model is licensed under
> the CC-BY-NC due to constraints from its training data (Emilia)."
> —— 出处：HF 模型库 `k2-fsa/ZipVoice` 的模型卡「License」节，仓库 `github.com/k2-fsa/ZipVoice`。

因此：

- 该权重 **不得用于任何商业目的**（含直接售卖、SaaS、商业产品内嵌等）。
- 你若要商用，**必须自行向权利方（k2-fsa / Next-gen Kaldi）取得单独授权**；Lunac 的 MIT 许可
  **不**涵盖该权重。
- 本项目本身是个人非商用项目，据此在公开渠道分发该权重。

## 三、署名（Attribution）

ZipVoice 权重按 CC BY-NC 4.0 的要求署名如下：

- **名称**：ZipVoice / ZipVoice-Distill（零样本语音克隆 TTS）。
- **作者 / 权利方**：Han Zhu 等（k2-fsa / Next-gen Kaldi 团队）。
- **来源**：模型 https://huggingface.co/k2-fsa/ZipVoice ，代码 https://github.com/k2-fsa/ZipVoice ，
  论文 "ZipVoice: Fast and High-Quality Zero-Shot Text-to-Speech with Flow Matching"，
  arXiv:2506.13053（https://arxiv.org/abs/2506.13053）。
- **许可**：CC BY-NC 4.0，https://creativecommons.org/licenses/by-nc/4.0/ 。
- **改动说明**：随本组件分发的权重是 k2-fsa/sherpa-onnx 官方发布的 ONNX 导出与 int8 量化版本
  （上游包 `zipvoice-distill-int8-zh-en-emilia`）；Lunac **未对其再做修改**，仅原样再分发。

## 四、espeak-ng 数据（GPL-3.0）与源码获取

`model/espeak-ng-data/` 为 eSpeak NG（espeak-ng）项目的数据文件，授权为 **GPL-3.0**。
GPL-3.0 要求分发二进制/数据时提供**获取对应源码的途径**：

- 上游项目地址：https://github.com/espeak-ng/espeak-ng （数据由项目源码构建/生成）。
- 数据文件的来源为 k2-fsa/sherpa-onnx 发布的 ZipVoice 模型包（`zipvoice-distill-int8-zh-en-emilia`）。
- 随本组件的该数据**未做修改**，原样再分发；对应源码请按上述项目地址获取（或其镜像 / 发行版打包）。

## 五、Vocos 权重（`vocos_24khz.onnx`）授权结论

`vocos_24khz.onnx` 为 Charactr 的 Vocos 声码器权重。经查证：

- 代码仓库 `gemelo-ai/vocos`（原 `charactr/vocos`）的 LICENSE 为 **MIT**（Copyright (c) 2023 Charactr Inc.）。
- 权重所在的 HF 模型库 `charactr/vocos-mel-24khz` 的模型卡 front-matter 标注的许可为 `license: mit`，
  且正文写有 "The code in this repository is released under the MIT license."。

结论：**按 MIT 处理**（见 `LICENSES/Vocos-MIT.txt`）。注意：模型卡原话针对"仓库中的代码"，
权重本身未被逐字单独声明；此处以上游模型卡的 MIT 标注为准，**如需严格商用仍建议向权利人复核**。
