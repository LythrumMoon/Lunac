// ── tuning-engine：调音插件的 DSP 引擎内核（backlog L5 / S0）────────────
//
// **分期口径**：S0 = 完全离线、可单测的那一块（WAV → DSP → WAV，FFT、滤波器、
// 配置 JSON），不碰系统音频、不要驱动、不要 UAC。实时/进程那份（stream-json 往返、
// 设备枚举、loopback 测量）是 S1/S2 —— 契约见 `docs/ai-spec.md` §4.10。
//
// 模块分工（每一块都能被测试直接调）：
//   · `fft`    —— 手写 radix-2 FFT（正 + 逆）+ 窗函数 + 幅度谱
//   · `biquad` —— 滤波器系数（RBJ cookbook）与逐样本处理，另给解析增益 `response_db()`
//   · `chain`  —— 按配置串起一条链（含 preamp / 延迟 / 声道复制），逐样本或整块处理
//   · `convolution` —— FFT 快速卷积（overlap-add）+ IR 读入 / 校验，EAPO 的 `Convolution` 内核
//   · `wav`    —— WAV 读写（PCM 16/24/32 位与 float32）
//   · `config` —— 链配置的 JSON 形态与**逐条合法校验**
//   · `measure`—— 把「链的频率响应」量出来（脉冲响应 → FFT → dB），供 `measure` 子命令与测试共用
//   · `gen`    —— 生成测试信号（正弦 / 脉冲 / 静音）
//
// **本文件是库根**：`src/main.rs` 只是 CLI 薄壳，所有算法都在这里，测试才调得到。

pub mod biquad;
pub mod chain;
pub mod config;
pub mod convolution;
pub mod fft;
pub mod gen;
pub mod measure;
pub mod wav;
