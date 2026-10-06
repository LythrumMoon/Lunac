// ── tuning_probe.rs — 调音的**实时硬件测量**（2026-10-02）────────────────────
//
// **它要回答的问题**：调音页上那条频响曲线是**宿主按 RBJ 解析式算出来的**（见
// ai-spec §4.6），它证明的是「这条链的数学是对的」，**不证明**「这些增益真的走到了
// 硬件」。中间还隔着 DSP 是否被挂上、系统是否又叠了别的音效、音量/静音开关等等。
// 所以这里做一次**真的测量**：播一段已知的扫频，用 WASAPI **回环**（loopback）把
// 输出设备正在渲染的那路信号录回来，两者相除就是「层层叠加之后、真正送给硬件的
// 那条频响」。
//
// **方法：指数扫频 + 双通道 FFT 相比，测两趟（2026-10-03 起）**
//   ① 生成一段对数扫频 x（20Hz→20kHz，两端各带线性淡入淡出，前后各留静音）；
//   ② **同一段 x 播两趟**：一趟**直通**（`player::plain_buffer`，量到的是**系统** ——
//      EAPO / 别的软件 / 设备）、一趟**走链**（`player::tuned_buffer` → `TuningSource`，
//      量到的是**最终输出**）；
//   ③ 每趟都回环录回来 mono 化成 y，先用互相关量出 y 相对 x 的延迟（设备延迟 / 缓冲深度
//      都是未知的），对齐后取扫频那一段，两边**加同一个 Hann 窗**做 FFT；
//   ④ 每趟各自 H(f) = Y(f)/X(f)。LTI 系统下这是**定义式**，等价于解卷积，但没有相位
//      对齐的坑；按 1/12 倍频程平滑，落到与面板同一套对数频点上；
//   ⑤ **仅自身链（实测）= 最终输出 ÷ 系统**（两趟逐点相减，都是 dB）。**只有它**能跟链的
//      合成曲线直接对齐 —— 拿「最终输出」去比会被系统音效污染（说不清差在哪）。
//      两趟的 `xpow` 逐位相同 ⇒ 频点网格与能量判据完全一致，相减不需要插值。
//
// **为什么回环而不是麦克风**：回环录的是「数字域里正要送去 DAC 的那一路」，于是
// 量到的差别只可能来自**软件链**（这正是用户要验证的东西）；用麦克风会把房间声学、
// 扬声器、麦克风频响全部混进来，反而说不出「调音有没有生效」。代价是**测不到扬声器
// 本身**——那需要另一套（近场麦克风）标定，不在本次范围内。
//
// **两条纪律**：
//   ① 采集的 COM 对象（`IMMDeviceEnumerator` / `IAudioClient` / `IAudioCaptureClient`）
//      **必须在自己那条线程上创建与销毁**（COM 套间模型），所以采集独占一条线程，
//      rodio 那侧留在调用线程 —— 不能反过来把 sink 搬过去（`cpal::Stream` 不是 Send）。
//   ② 输出设备采样率以 **rodio sink 自己的 config** 为准（扫频必须按它生成）；
//      采集侧拿到混音格式后要**核对**，不一致就直接报错，而不是硬着头皮量出一条错的曲线。

use std::f64::consts::PI;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tuning_engine::fft::{fft_in_place, hann_window, next_pow2};

use crate::player;

// ── 测试信号与频点 ───────────────────────────────────────────────
const SWEEP_F0: f64 = 20.0;
const SWEEP_F1: f64 = 20_000.0;
/// 扫频时长。**1.5s 不是随便定的**：低频那端的分辨率受窗长限制，太短的扫频在
/// 100Hz 以下会量出假的起伏（那看起来很像「滤波器有问题」，会把人带偏）。
const SWEEP_SECS: f64 = 1.5;
/// 扫频前/后各留一段静音：前留给设备缓冲延迟（免得开头被吃掉），后留给尾巴。
const LEAD_SECS: f64 = 0.25;
const TAIL_SECS: f64 = 0.6;
/// 幅度。**刻意压低**：这次出声是用户主动点的，但也没必要用满量程吓人一跳；
/// 0.3 在回环里本底已经足够低（数字域里没有噪声floor的问题）。
const SWEEP_AMP: f64 = 0.3;
const FADE_SECS: f64 = 0.02;

/// 对外报的频点范围：比扫频两端各收一点（两端有淡入淡出，能量偏低，量出来不可信）。
const BAND_LO: f64 = 30.0;
const BAND_HI: f64 = 18_000.0;
/// 频点数：与面板那条合成曲线同量级，画出来是同一根线。
const POINTS: usize = 240;
/// 平滑带宽（倍频程，总宽）。1/12 是 REW / Peace 那一族的默认口径。
const SMOOTH_OCT: f64 = 1.0 / 12.0;

// ── 结果 ─────────────────────────────────────────────────────────

/// 一次实时测量的结果（**两条实测曲线 + 一条解析曲线**，2026-10-03 起测两趟）。
///
/// `freqs` 的长度**可能少于 `POINTS`** —— 扫频两端（<30Hz / >18kHz）与能量过低的点
/// 会被摘掉，前端按拿到的点直接画折线。下面三条曲线的下标都对着 `freqs`。
#[derive(Debug, Serialize, Clone)]
pub struct MeasuredResponse {
    pub sample_rate: f64,
    pub freqs: Vec<f64>,
    /// **最终输出**（实测）：走链那一趟的 `Y/X` —— 链 × **系统**（EAPO / 设备 / 别的
    /// 音效都在里面，因为回环录的是默认渲染端点正在送出去的那一路）。
    pub db: Vec<f64>,
    /// **仅自身链**（实测）：最终输出 ÷ 系统那一趟。**这才是「调音到底生效了没有」**
    /// 该跟 `ref_db` 对比的那条 —— 拿 `db` 去比会被系统音效污染。
    pub chain_db: Vec<f64>,
    /// 同频点上的**链合成**（宿主解析式）曲线，与 `db` / `chain_db` 一一对应。
    pub ref_db: Vec<f64>,
    /// 有效率（拿到的点 / `POINTS`）。低于 ~0.7 说明这次测量没覆盖住整个频段，
    /// 界面上要如实提示，而不是把一条残缺的线当成完整的。
    pub coverage: f64,
    pub elapsed_ms: u64,
}

// ── 入口 ─────────────────────────────────────────────────────────

/// 跑一次完整测量（**两趟**：一趟直通量「系统」、一趟走链量「最终输出」）。
/// **阻塞**（命令层套 `run_blocking`），整段约 6 秒。
///
/// **为什么要两趟**：`db`（最终输出）里混着系统音效（EAPO / 别的软件 / 设备），拿它去
/// 跟「链的合成曲线」比，差出来的东西说不清是链错了还是别的软件在掺和。两趟相除把系统
/// 剥掉，才有 **仅自身链（实测）** 这条真正该跟合成曲线对齐的线。
pub fn measure() -> Result<MeasuredResponse, String> {
    let t0 = Instant::now();

    // ① 播放侧先就位：设备必须在**当前线程**开（见文件头纪律 ①），顺便问到采样率。
    let mut sink = rodio::DeviceSinkBuilder::open_default_sink()
        .map_err(|_| "ERR_NO_AUDIO_DEVICE".to_string())?;
    sink.log_on_drop(false);
    let rate = sink.config().sample_rate().get() as f64;
    let channels = sink.config().channel_count().get();
    let player = rodio::Player::connect_new(sink.mixer());
    // 音量必须 1.0：回环里的电平和它是乘性关系，量的是「链」而不是「音量」。
    player.set_volume(1.0);
    let mono = make_sweep(rate);

    // ② 出声前把我们自己正在放的那一首按下去 —— 否则录到的是「歌 + 扫频」的混合。
    let was_playing = player::pause_for_probe();

    // ③ 两趟：同一段扫频，唯一差别是**走不走链**。
    let passes = (|| -> Result<(Vec<f64>, Vec<f64>), String> {
        // 一趟：起采集（另起线程，COM 对象不能跨线程）→ 等它就绪 → 出声 → 收采集。
        // `ready` 保证「采集已经真的开始」之后才出声，否则开头一段扫频不在录音里，
        // 互相关会找不到峰。
        let one_pass = |through_chain: bool| -> Result<Vec<f64>, String> {
            let secs = LEAD_SECS + SWEEP_SECS + TAIL_SECS + 0.4;
            let (ready_rx, capture) = spawn_capture(secs, rate);
            match ready_rx.recv_timeout(Duration::from_secs(8)) {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    let _ = capture.join();
                    return Err(e);
                }
                Err(_) => {
                    let _ = capture.join();
                    return Err("打开回环采集超时".to_string());
                }
            }
            let inter: Vec<f32> = mono
                .iter()
                .flat_map(|v| std::iter::repeat(*v as f32).take(channels as usize))
                .collect();
            if through_chain {
                player.append(player::tuned_buffer(inter, channels, rate as u32));
            } else {
                player.append(player::plain_buffer(inter, channels, rate as u32));
            }
            player.sleep_until_end();
            // 采集按自己的 deadline 结束，join 顶多多等那 0.4s 余量。
            capture.join().map_err(|_| "采集线程崩了".to_string())?
        };
        Ok((one_pass(false)?, one_pass(true)?))
    })();

    drop(player);
    drop(sink);
    // **失败也要把歌放回去** —— 否则一次没测成的测量会把音乐一直按在暂停上。
    player::resume_after_probe(was_playing);
    let (y_sys, y_total) = passes?;

    // ④ 相比 → 三条曲线。两趟用**同一段 `x`**，而 `xpow` 只取决于 `x` ⇒ 频点网格与
    //    能量判据两趟完全一致，直接相减即可。
    let p_sys = measure_powers(&mono, &y_sys, rate)?;
    let p_tot = measure_powers(&mono, &y_total, rate)?;
    let (freqs, db, coverage) = collapse_default(&p_tot)?;
    let sys_db = collapse_at(&p_sys, &freqs);
    let chain_db: Vec<f64> = db.iter().zip(&sys_db).map(|(total, sys)| total - sys).collect();
    let ref_db = player::tuning_reference_db(&freqs, rate);

    Ok(MeasuredResponse {
        sample_rate: rate,
        freqs,
        db,
        chain_db,
        ref_db,
        coverage,
        elapsed_ms: t0.elapsed().as_millis() as u64,
    })
}

// ── 测试信号 ─────────────────────────────────────────────────────

/// 指数（对数）扫频 + 前后静音。返回**单声道**样本。
///
/// 相位不是「频率×时间」而是**积分**：`φ(t) = 2π·f0·T/k·(e^{kt/T} − 1)`，`k = ln(f1/f0)`。
/// 直接写 `2π·f(t)·t` 是常见错误 —— 扫频的瞬时频率会变成两倍，量出来的曲线整个是错的。
fn make_sweep(rate: f64) -> Vec<f64> {
    let n_lead = (LEAD_SECS * rate).round() as usize;
    let n_sweep = (SWEEP_SECS * rate).round() as usize;
    let n_tail = (TAIL_SECS * rate).round() as usize;
    let n_fade = ((FADE_SECS * rate).round() as usize).max(1);
    let mut out = vec![0.0f64; n_lead + n_sweep + n_tail];
    let k = (SWEEP_F1 / SWEEP_F0).ln();
    let scale = 2.0 * PI * SWEEP_F0 * SWEEP_SECS / k;
    for i in 0..n_sweep {
        let frac = i as f64 / n_sweep as f64;
        let phase = scale * ((k * frac).exp() - 1.0);
        // 线性淡入淡出：不放的话两端是硬切，频谱会拖出一片假的宽带能量。
        let env = if i < n_fade {
            i as f64 / n_fade as f64
        } else if i + n_fade >= n_sweep {
            (n_sweep - 1 - i) as f64 / n_fade as f64
        } else {
            1.0
        };
        out[n_lead + i] = SWEEP_AMP * env * phase.sin();
    }
    out
}

// ── 采集（WASAPI 回环）────────────────────────────────────────────

/// 起一条采集线程，返回「就绪信号」与 join 句柄（句柄给出 mono 化的样本）。
///
/// 就绪信号发的是 `Ok(())` **在 `Start()` 成功之后** —— 调用方据此才出声。
fn spawn_capture(
    secs: f64,
    expected_rate: f64,
) -> (
    mpsc::Receiver<Result<(), String>>,
    std::thread::JoinHandle<Result<Vec<f64>, String>>,
) {
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let handle = std::thread::spawn(move || match capture_blocking(secs, expected_rate, &tx) {
        Ok(v) => Ok(v),
        Err(e) => {
            let _ = tx.send(Err(e.clone()));
            Err(e)
        }
    });
    (rx, handle)
}

fn capture_blocking(
    secs: f64,
    expected_rate: f64,
    ready: &mpsc::Sender<Result<(), String>>,
) -> Result<Vec<f64>, String> {
    // SAFETY: 本函数整段是 COM / WASAPI 的裸调用；指针都来自系统分配、长度由
    // `nBlockAlign` 与 `frames` 界定，且每次 `GetBuffer` 都配对了 `ReleaseBuffer`。
    unsafe {
        // 多线程套间。`RPC_E_CHANGED_MODE`（这个线程已在别的套间）不算失败 ——
        // 照样能用，只是这次不该 `CoUninitialize`。
        let hr = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_MULTITHREADED,
        );
        let result = capture_inner(secs, expected_rate, ready);
        if hr.is_ok() {
            windows::Win32::System::Com::CoUninitialize();
        }
        result
    }
}

unsafe fn capture_inner(
    secs: f64,
    expected_rate: f64,
    ready: &mpsc::Sender<Result<(), String>>,
) -> Result<Vec<f64>, String> {
    use windows::Win32::Media::Audio::*;
    use windows::Win32::System::Com::*;

    let enumerator: IMMDeviceEnumerator =
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
            .map_err(|e| format!("枚举音频设备失败：{e}"))?;
    let device = enumerator
        .GetDefaultAudioEndpoint(eRender, eConsole)
        .map_err(|e| format!("取默认输出设备失败：{e}"))?;
    let client: IAudioClient = device
        .Activate(CLSCTX_ALL, None)
        .map_err(|e| format!("打开输出设备失败：{e}"))?;
    let pwfx = client
        .GetMixFormat()
        .map_err(|e| format!("读设备混音格式失败：{e}"))?;

    let fmt = *pwfx;
    let channels = fmt.nChannels as usize;
    let rate = fmt.nSamplesPerSec as f64;
    let frame_bytes = fmt.nBlockAlign as usize;
    let bits = fmt.wBitsPerSample as usize;
    let tag = fmt.wFormatTag as u32;

    // 样本格式：`WAVE_FORMAT_EXTENSIBLE`(0xFFFE) 要看 SubFormat 才算得准；
    // 直接认 wFormatTag 只在老式非扩展格式上成立。
    let mut is_float = tag == WAVE_FORMAT_IEEE_FLOAT as u32;
    let mut is_pcm = tag == WAVE_FORMAT_PCM;
    if tag == WAVE_FORMAT_EXTENSIBLE as u32 && fmt.cbSize >= 22 {
        let we = core::ptr::read_unaligned(pwfx as *const WAVEFORMATEXTENSIBLE);
        let sub = we.SubFormat;
        if sub == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT {
            is_float = true;
            is_pcm = false;
        } else if sub == KSDATAFORMAT_SUBTYPE_PCM {
            is_float = false;
            is_pcm = true;
        }
    }

    // 采样率必须与扫频一致，否则「Y/X」比的是两个不同时间轴上的东西。
    if (rate - expected_rate).abs() > 1.0 || channels == 0 || frame_bytes == 0 {
        CoTaskMemFree(Some(pwfx as *const _));
        return Err(format!(
            "输出设备的混音格式无法用于测量（{rate:.0}Hz / {channels} 声道 / tag={tag}，\
             播放侧是 {expected_rate:.0}Hz）"
        ));
    }
    if !is_float && !is_pcm {
        CoTaskMemFree(Some(pwfx as *const _));
        return Err(format!("输出设备的混音格式不支持（tag={tag}，{bits} 位）"));
    }

    // 共享模式下缓冲时长必须非零；200ms 对回环足够宽裕（它只是采集侧的积压深度，
    // 不影响我们对齐 —— 对齐靠互相关，不靠时间戳）。
    let hns_buffer: i64 = 2_000_000;
    let init = client.Initialize(
        AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_LOOPBACK,
        hns_buffer,
        0,
        pwfx,
        None,
    );
    CoTaskMemFree(Some(pwfx as *const _));
    init.map_err(|e| format!("初始化回环采集失败：{e}"))?;

    let capture: IAudioCaptureClient = client
        .GetService()
        .map_err(|e| format!("取采集接口失败：{e}"))?;
    client.Start().map_err(|e| format!("启动回环采集失败：{e}"))?;
    // 到这里采集真的在跑了 —— 放行播放。
    let _ = ready.send(Ok(()));

    let bytes_per_sample = (bits / 8).max(1);
    let mut out: Vec<f64> = Vec::with_capacity((secs * rate) as usize + 4096);
    let deadline = Instant::now() + Duration::from_secs_f64(secs);

    while Instant::now() < deadline {
        let packet = capture.GetNextPacketSize().unwrap_or(0);
        if packet == 0 {
            std::thread::sleep(Duration::from_millis(4));
            continue;
        }
        let mut data: *mut u8 = std::ptr::null_mut();
        let mut frames: u32 = 0;
        let mut flags: u32 = 0;
        if capture
            .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
            .is_err()
        {
            break;
        }
        let silent = flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;
        if !silent && !data.is_null() && frames > 0 {
            for i in 0..frames as usize {
                let base = i * frame_bytes;
                let mut acc = 0.0f64;
                for c in 0..channels {
                    let p = data.add(base + c * bytes_per_sample);
                    acc += decode_one(p, is_float, bytes_per_sample);
                }
                out.push(acc / channels as f64);
            }
        }
        let _ = capture.ReleaseBuffer(frames);
    }
    let _ = client.Stop();
    Ok(out)
}

/// 读一个样本并归一到 ±1.0。`is_float` 时按 IEEE float32/64；否则按定点 PCM。
unsafe fn decode_one(p: *const u8, is_float: bool, bytes: usize) -> f64 {
    if is_float {
        if bytes == 4 {
            std::ptr::read_unaligned(p as *const f32) as f64
        } else if bytes == 8 {
            std::ptr::read_unaligned(p as *const f64)
        } else {
            0.0
        }
    } else if bytes == 2 {
        std::ptr::read_unaligned(p as *const i16) as f64 / 32768.0
    } else if bytes == 4 {
        std::ptr::read_unaligned(p as *const i32) as f64 / 2_147_483_648.0
    } else if bytes == 3 {
        // 24 位 PCM：没有对齐的读法，逐字节拼。
        let b0 = *p as u32;
        let b1 = *p.add(1) as u32;
        let b2 = *p.add(2) as u32;
        let raw = (b0 | (b1 << 8) | (b2 << 16)) as i32;
        let signed = if raw & 0x0080_0000 != 0 { raw | !0x00FF_FFFF } else { raw };
        signed as f64 / 8_388_608.0
    } else {
        0.0
    }
}

// ── 相干比较 ─────────────────────────────────────────────────────

/// `WAVE_FORMAT_EXTENSIBLE` 的两个标准子格式 GUID。
/// **自己写死而不是找常量**：`windows` 0.58 的 `Media_Audio` 里没有导出它们。
const KSDATAFORMAT_SUBTYPE_PCM: windows::core::GUID =
    windows::core::GUID::from_u128(0x0000_0001_0000_0010_8000_00aa_0038_9b71);
const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: windows::core::GUID =
    windows::core::GUID::from_u128(0x0000_0003_0000_0010_8000_00aa_0038_9b71);

/// 非回环格式用不了的两个 tag（`WAVE_FORMAT_EXTENSIBLE` 是 0xFFFE）。
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;

/// 一趟测量的中间量：**已对齐、已加窗**的双通道功率谱。
///
/// `xpow` 只取决于扫频本身（与 `y` 的延迟无关）⇒ **两趟的 `xpow` 逐位相同**，
/// 于是「最终输出 ÷ 系统」这条比值在每个频点上都成立（见 `measure`），
/// `band_db` 里那条「参考能量太低」的判据在两趟上也必然给出同样的结果。
struct Powers {
    xpow: Vec<f64>,
    ypow: Vec<f64>,
    /// 频率分辨率（Hz/格）。
    df: f64,
    /// 单边谱的格数。
    half: usize,
    /// `xpow` 的峰值（能量判据用）。
    xmax: f64,
}

/// 一趟测量：互相关对齐 → 取扫频那一段 → 两边同窗 FFT → 功率谱。
fn measure_powers(x: &[f64], y: &[f64], rate: f64) -> Result<Powers, String> {
    if x.len() < 1024 || y.len() < 1024 {
        return Err("录回来的样本太少，量不出曲线".to_string());
    }

    // ① 互相关找延迟：corr = IFFT(conj(X)·Y)，峰位就是 y 比 x 晚的样本数。
    //    长度取 `next_pow2(len_x + len_y)` 是为了**线性**相关（短一圈会绕回来，
    //    峰可能出现在错误的圈上）。
    let n = next_pow2(x.len() + y.len());
    let mut xr = vec![0.0f64; n];
    let mut xi = vec![0.0f64; n];
    let mut yr = vec![0.0f64; n];
    let mut yi = vec![0.0f64; n];
    xr[..x.len()].copy_from_slice(x);
    yr[..y.len()].copy_from_slice(y);
    fft_in_place(&mut xr, &mut xi)?;
    fft_in_place(&mut yr, &mut yi)?;
    let mut cr = vec![0.0f64; n];
    let mut ci = vec![0.0f64; n];
    for k in 0..n {
        let (xre, xim) = (xr[k], xi[k]);
        let (yre, yim) = (yr[k], yi[k]);
        // conj(X)·Y
        cr[k] = xre * yre + xim * yim;
        ci[k] = xre * yim - xim * yre;
    }
    inverse_fft(&mut cr, &mut ci)?;
    let mut delay = 0usize;
    let mut best = -1.0f64;
    for k in 0..n {
        let v = cr[k] * cr[k] + ci[k] * ci[k];
        if v > best {
            best = v;
            delay = k;
        }
    }

    // ② 对齐后取扫频那一段（对齐点 + 前面那段静音 = 扫频起点）。
    let lead = (LEAD_SECS * rate).round() as usize;
    let seg = (SWEEP_SECS * rate).round() as usize;
    let start = delay + lead;
    if start + seg > y.len() {
        return Err(format!(
            "设备延迟太大（{delay} 样本），录回来的长度不够分析这次扫频"
        ));
    }
    let xs = &x[lead..lead + seg];
    let ys = &y[start..start + seg];

    // ③ 同一个 Hann 窗、同一段长度。LTI 下 H = Y/X 与窗无关（两边同窗），
    //    加窗只是为了压泄漏、把带外噪声的权重降下去。
    let w = hann_window(seg);
    let n2 = next_pow2(seg * 2);
    let mut xr = vec![0.0f64; n2];
    let mut xi = vec![0.0f64; n2];
    let mut yr = vec![0.0f64; n2];
    let mut yi = vec![0.0f64; n2];
    for i in 0..seg {
        xr[i] = xs[i] * w[i];
        yr[i] = ys[i] * w[i];
    }
    fft_in_place(&mut xr, &mut xi)?;
    fft_in_place(&mut yr, &mut yi)?;

    let half = n2 / 2 + 1;
    let mut xpow = vec![0.0f64; half];
    let mut ypow = vec![0.0f64; half];
    for k in 0..half {
        xpow[k] = xr[k] * xr[k] + xi[k] * xi[k];
        ypow[k] = yr[k] * yr[k] + yi[k] * yi[k];
    }
    let xmax = xpow.iter().cloned().fold(0.0f64, f64::max);
    if xmax <= 0.0 {
        return Err("测试信号没发出去（采集里全是静音）".to_string());
    }

    Ok(Powers {
        xpow,
        ypow,
        df: rate / n2 as f64,
        half,
        xmax,
    })
}

/// 一个频点上的增益（dB）：在该点 ±1/12 倍频程内做**功率**平均再相比。
///
/// 功率平均（而不是 dB 平均）才是对的：dB 平均会把陷波的零点抬起来。
/// 返回 `None` = 参考能量太低（扫频没覆盖到）⇒ 这一格不参与，除出来的是噪声。
fn band_db(p: &Powers, f: f64) -> Option<f64> {
    let half_band = 2.0f64.powf(SMOOTH_OCT / 2.0);
    let k1 = ((f / half_band) / p.df).floor().max(1.0) as usize;
    let k2 = (((f * half_band) / p.df).ceil() as usize).min(p.half - 1);
    if k2 < k1 {
        return None;
    }
    let mut sx = 0.0f64;
    let mut sy = 0.0f64;
    for k in k1..=k2 {
        if p.xpow[k] >= p.xmax * 1e-6 {
            sx += p.xpow[k];
            sy += p.ypow[k];
        }
    }
    if sx <= 0.0 || sy <= 0.0 {
        return None;
    }
    Some(10.0 * (sy / sx).log10())
}

/// 塌到与面板同一套对数频点上，返回 `(freqs, db, coverage)`（带「能量太低就摘掉」的判据）。
fn collapse_default(p: &Powers) -> Result<(Vec<f64>, Vec<f64>, f64), String> {
    let mut freqs = Vec::with_capacity(POINTS);
    let mut db = Vec::with_capacity(POINTS);
    for i in 0..POINTS {
        let f = BAND_LO * (BAND_HI / BAND_LO).powf(i as f64 / (POINTS - 1) as f64);
        if let Some(d) = band_db(p, f) {
            freqs.push(f);
            db.push(d);
        }
    }
    if freqs.len() < 8 {
        return Err("扫频回来的信号太弱，量不出曲线（请确认输出设备有声、音量不为 0）".to_string());
    }
    let coverage = freqs.len() as f64 / POINTS as f64;
    Ok((freqs, db, coverage))
}

/// 在**给定频点**上塌陷。这些频点取自另一趟的有效集合 ⇒ 参考能量必然够
/// （两趟 `xpow` 相同，见 `Powers`），所以这里的 `None` 理论上不会出现；
/// 真出现了就按「系统在这一格没影响」算（0dB），不让一个点毁掉整条曲线。
fn collapse_at(p: &Powers, freqs: &[f64]) -> Vec<f64> {
    freqs.iter().map(|f| band_db(p, *f).unwrap_or(0.0)).collect()
}

/// `mono_x`（我们生成的扫频）与 `mono_y`（录回来的）→ 频响（**单趟**的便捷形态）。
///
/// 返回 `(freqs, db, coverage)`；`db` 是相对量（Y/X），**不做任何归一化** ——
/// 回环是数字域里的直通，所以这个比值本身就是「链 + 系统」的增益。
///
/// ⚠️ **只有测试用它**（生产路径走 `measure_powers` + `collapse_default` / `collapse_at`
/// 那一对 —— 两趟要用**同一套频点**，见 `measure`）。
#[cfg(test)]
fn transfer_db(x: &[f64], y: &[f64], rate: f64) -> Result<(Vec<f64>, Vec<f64>, f64), String> {
    collapse_default(&measure_powers(x, y, rate)?)
}

/// 逆 FFT：`IFFT(X) = conj(FFT(conj(X))) / N`。
fn inverse_fft(re: &mut [f64], im: &mut [f64]) -> Result<(), String> {
    let n = re.len();
    for v in im.iter_mut() {
        *v = -*v;
    }
    fft_in_place(re, im)?;
    let inv = 1.0 / n as f64;
    for v in re.iter_mut() {
        *v *= inv;
    }
    for v in im.iter_mut() {
        *v = -*v * inv;
    }
    Ok(())
}

// ── 命令 ─────────────────────────────────────────────────────────

/// 实时测一次「最终送到硬件的频响」。**会出声**（约 1.5 秒扫频），命令层已
/// `run_blocking`，整段约 3 秒。
#[tauri::command]
pub async fn player_tuning_measure() -> Result<MeasuredResponse, String> {
    crate::commands::run_blocking(measure).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 扫频的相位必须是**积分**出来的：瞬时频率从 f0 单调走到 f1。
    /// 这条钉住「把相位写成 2π·f(t)·t」那个经典错误（那样瞬时频率会翻倍）。
    #[test]
    fn sweep_moves_monotonically_from_f0_to_f1() {
        let rate = 48000.0;
        let x = make_sweep(rate);
        let lead = (LEAD_SECS * rate).round() as usize;
        let seg = (SWEEP_SECS * rate).round() as usize;

        // 过零计数估瞬时频率：前半段与后半段各数一遍。
        let count = |from: usize, to: usize| -> f64 {
            let mut n = 0usize;
            for i in from + 1..to {
                if x[i - 1] <= 0.0 && x[i] > 0.0 {
                    n += 1;
                }
            }
            n as f64
        };
        let quarter = seg / 4;
        let f_early = count(lead, lead + quarter) as f64 / (quarter as f64 / rate);
        let f_late =
            count(lead + seg - quarter, lead + seg) as f64 / (quarter as f64 / rate);
        assert!(
            f_late > f_early * 5.0,
            "扫频的频率没有随时间是上升的（前 {f_early:.0}Hz / 后 {f_late:.0}Hz）"
        );
        // 两端静音（淡入淡出的存在保证不会有硬切）
        assert!(x[0] == 0.0 && x[x.len() - 1] == 0.0, "两端必须是静音");
    }

    /// 把一段已知增益的「系统」套上去，双通道相比要能把它量回来。
    /// 用**一阶低通**而不是别的东西：它有解析解，且高频端掉得明显，好判。
    #[test]
    fn transfer_db_recovers_a_known_filter() {
        let rate = 48000.0;
        let x = make_sweep(rate);
        // 简单一阶 IIR：y[n] = a·y[n-1] + (1-a)·x[n]（截止约 1kHz）
        let fc = 1000.0f64;
        let a = (-2.0 * PI * fc / rate).exp();
        let mut y = vec![0.0f64; x.len() + (0.2 * rate) as usize];
        let mut st = 0.0f64;
        for i in 0..y.len() {
            let xi = if i < x.len() { x[i] } else { 0.0 };
            st = a * st + (1.0 - a) * xi;
            y[i] = st;
        }
        let (freqs, db, cov) = transfer_db(&x, &y, rate).unwrap();
        assert!(cov > 0.8, "有效率只有 {cov}");
        for (f, d) in freqs.iter().zip(db.iter()) {
            let theory = 20.0
                * ((1.0 - a) / (1.0 - 2.0 * a * (2.0 * PI * f / rate).cos() + a * a).sqrt()).log10();
            assert!(
                (d - theory).abs() < 1.0,
                "{f:.0}Hz：实测 {d:.2}dB vs 理论 {theory:.2}dB"
            );
        }
    }

    /// 一阶低通的时域实现（测试里当「系统」/「链」用 —— 它有解析解，好判）。
    fn lp_filter(fc: f64, input: &[f64], rate: f64, extra: usize) -> Vec<f64> {
        let a = (-2.0 * PI * fc / rate).exp();
        let mut y = vec![0.0; input.len() + extra];
        let mut st = 0.0f64;
        for i in 0..y.len() {
            let xi = if i < input.len() { input[i] } else { 0.0 };
            st = a * st + (1.0 - a) * xi;
            y[i] = st;
        }
        y
    }

    /// 一阶低通在 `f` 处的解析增益（dB）。
    fn lp_db(fc: f64, f: f64, rate: f64) -> f64 {
        let a = (-2.0 * PI * fc / rate).exp();
        let w = 2.0 * PI * f / rate;
        20.0 * ((1.0 - a) / (1.0 - 2.0 * a * w.cos() + a * a).sqrt()).log10()
    }

    /// **两趟相减**是 P4 的核心：造一个「系统」（1kHz 低通）+ 一个「链」（3kHz 低通，
    /// 串在系统**之后**）⇒ `db`（最终输出）量到两者的串联，`chain_db`（仅自身链）
    /// 只应当量到**链自己**。这条错了，界面上那条「仅自身链」就是假的。
    #[test]
    fn two_pass_subtraction_isolates_the_chain_from_the_system() {
        let rate = 48000.0;
        let x = make_sweep(rate);
        let extra = (0.2 * rate) as usize;
        // 「系统」：x 过一遍 1kHz 低通；「最终输出」：再串一遍 3kHz 低通
        let y_sys = lp_filter(1000.0, &x, rate, extra);
        let y_total = lp_filter(3000.0, &y_sys, rate, extra);

        let p_sys = measure_powers(&x, &y_sys, rate).unwrap();
        let p_tot = measure_powers(&x, &y_total, rate).unwrap();
        let (freqs, db, cov) = collapse_default(&p_tot).unwrap();
        assert!(cov > 0.8, "有效率只有 {cov}");
        let sys_db = collapse_at(&p_sys, &freqs);
        let chain_db: Vec<f64> = db.iter().zip(&sys_db).map(|(t, s)| t - s).collect();

        for (i, f) in freqs.iter().enumerate() {
            let total_theory = lp_db(1000.0, *f, rate) + lp_db(3000.0, *f, rate);
            assert!(
                (db[i] - total_theory).abs() < 1.5,
                "@{f:.0}Hz 最终输出：实测 {:.2} vs 理论 {total_theory:.2}",
                db[i]
            );
            let chain_theory = lp_db(3000.0, *f, rate);
            assert!(
                (chain_db[i] - chain_theory).abs() < 1.5,
                "@{f:.0}Hz 仅自身链：实测 {:.2} vs 理论 {chain_theory:.2}",
                chain_db[i]
            );
        }
    }

    #[test]
    fn inverse_fft_round_trips() {
        let n = 16usize;
        let orig: Vec<f64> = (0..n).map(|i| (i as f64 * 0.37).sin()).collect();
        let mut re = orig.clone();
        let mut im = vec![0.0f64; n];
        fft_in_place(&mut re, &mut im).unwrap();
        inverse_fft(&mut re, &mut im).unwrap();
        for i in 0..n {
            assert!((re[i] - orig[i]).abs() < 1e-9, "第 {i} 个样本对不上");
        }
    }
}
