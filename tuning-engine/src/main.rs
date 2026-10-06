// ── CLI 薄壳 ────────────────────────────────────────────────────
// 算法全在库里（`tuning_engine::*`），这里只做参数解析与打印 —— 这样每条行为都能
// 被 `cargo test` 直接调用（见 ai-spec §16：测试要打真实现，不要只测一层壳）。
//
// 子命令契约见 `docs/ai-spec.md` §4.10。
//
// **四个子命令对应四件事**：`check` 校验配置、`gen` 造已知信号、`render` 跑一遍链、
// `measure` 把结果量出来。**面板（S1）不走这里** —— 面板与引擎之间是 NDJSON
// （照抄 `agent.exe` 的范式）；这里是人手用的那一面，输出是给人看的。
//
// 退出码：0 成功 / 1 运行失败（文件读不了、配置非法、音频不合法）/ 2 用法错误。

use std::process::ExitCode;

use tuning_engine::chain::{self, Chain};
use tuning_engine::config::ChainConfig;
use tuning_engine::gen;
use tuning_engine::measure;
use tuning_engine::wav::{self, Audio};

const USAGE: &str = "\
tuning-engine —— 调音引擎（S0：离线 WAV → DSP → WAV）

用法：
  tuning-engine check   <config.json> [--rate N] [--fft N]
      校验配置并打印链；再把「理论频响」与「实测频响」对拍（判据 ±0.1dB）。
  tuning-engine measure <wav> [--ref <wav>]
      量这个 WAV 的主频与电平；给了 --ref 就报「相对参考的增益」。
  tuning-engine gen     -o <out.wav> [--kind tone|silence] [--rate N] [--freq F]
                                     [--seconds S] [--amp A] [--channels C]
      生成一段答案已知的信号（验收的起点）。
  tuning-engine render  -c <config.json> -i <in.wav> -o <out.wav>
      按配置处理一段音频，并报峰值 / 越界样本数 / 主频处的链增益。

默认值：--rate 48000、--fft 32768、--freq 1000、--seconds 1、--amp 0.5、--channels 1

退出码：0 成功 / 1 运行失败 / 2 用法错误。";

/// 失败分两类 —— 分开是为了让退出码有意义（脚本能区分「我调错了」与「东西不对」）。
enum Fail {
    Usage(String),
    Run(String),
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Fail::Usage(m)) => {
            eprintln!("用法错误：{m}\n\n{USAGE}");
            ExitCode::from(2)
        }
        Err(Fail::Run(m)) => {
            eprintln!("失败：{m}");
            ExitCode::from(1)
        }
    }
}

fn run(args: &[String]) -> Result<(), Fail> {
    let Some(cmd) = args.first() else {
        return Err(Fail::Usage("缺少子命令".to_string()));
    };
    let rest = &args[1..];
    if cmd == "-h" || cmd == "--help" || cmd == "help" {
        println!("{USAGE}");
        return Ok(());
    }
    match cmd.as_str() {
        "check" => cmd_check(rest),
        "measure" => cmd_measure(rest),
        "gen" => cmd_gen(rest),
        "render" => cmd_render(rest),
        other => Err(Fail::Usage(format!("未知子命令 {other:?}"))),
    }
}

// ── 参数解析 ───────────────────────────────────────────────────

struct Args {
    pos: Vec<String>,
    flags: Vec<(String, String)>,
}

impl Args {
    fn get(&self, name: &str) -> Option<&str> {
        self.flags
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    fn num(&self, name: &str, default: f64) -> Result<f64, Fail> {
        match self.get(name) {
            None => Ok(default),
            Some(v) => v
                .parse::<f64>()
                .map_err(|_| Fail::Usage(format!("{name} 需要一个数，收到 {v:?}"))),
        }
    }

    fn uint(&self, name: &str, default: u64) -> Result<u64, Fail> {
        match self.get(name) {
            None => Ok(default),
            Some(v) => v
                .parse::<u64>()
                .map_err(|_| Fail::Usage(format!("{name} 需要一个非负整数，收到 {v:?}"))),
        }
    }

    fn need_pos(&self, what: &str, i: usize) -> Result<&str, Fail> {
        self.pos
            .get(i)
            .map(|s| s.as_str())
            .ok_or_else(|| Fail::Usage(format!("缺一个参数：{what}")))
    }

    fn extra_pos(&self, allowed: usize) -> Result<(), Fail> {
        if self.pos.len() > allowed {
            return Err(Fail::Usage(format!(
                "多了用不上的位置参数：{}",
                self.pos[allowed..].join(" ")
            )));
        }
        Ok(())
    }
}

/// 极简解析：`--name value` 取值，其余（不以 `-` 开头）当位置参数。
///
/// **`value_flags` 之外的 `--xxx` 一律报错** —— 拼错的参数被静默忽略是最气人的一类 bug
/// （用户以为开了某个开关，其实没有）。
fn parse(rest: &[String], value_flags: &[&str]) -> Result<Args, Fail> {
    let mut pos = Vec::new();
    let mut flags = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let a = &rest[i];
        if a.starts_with('-') {
            if !value_flags.contains(&a.as_str()) {
                return Err(Fail::Usage(format!(
                    "未知参数 {a:?}（这个子命令可用：{}）",
                    value_flags.join(" / ")
                )));
            }
            let v = rest
                .get(i + 1)
                .ok_or_else(|| Fail::Usage(format!("{a} 后面缺一个值")))?;
            flags.push((a.clone(), v.clone()));
            i += 2;
        } else {
            pos.push(a.clone());
            i += 1;
        }
    }
    Ok(Args { pos, flags })
}

// ── 读文件的小工具（失败要给「是哪个路径」）─────────────────────

fn read_config(path: &str, sample_rate: f64) -> Result<ChainConfig, Fail> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| Fail::Run(format!("读不了配置 {path}：{e}")))?;
    ChainConfig::from_json(&text, sample_rate)
        .map_err(|e| Fail::Run(format!("配置 {path} 不合法：{e}")))
}

fn read_wav(path: &str) -> Result<Audio, Fail> {
    wav::read(std::path::Path::new(path))
        .map_err(|e| Fail::Run(format!("读不了音频 {path}：{e}")))
}

fn write_wav(path: &str, a: &Audio) -> Result<(), Fail> {
    wav::write(std::path::Path::new(path), a)
        .map_err(|e| Fail::Run(format!("写不了音频 {path}：{e}")))
}

/// 多声道按**平均**折成单声道。
///
/// `tone_level` 量的是音调（不是声场），所以折单声道是对的；用平均而不是取第一声道，
/// 是为了「只有右声道有信号」这种素材也能量出来。
fn mono(a: &Audio) -> Vec<f64> {
    let ch = a.channels.max(1) as usize;
    if ch == 1 {
        return a.samples.clone();
    }
    a.samples
        .chunks(ch)
        .map(|f| f.iter().sum::<f64>() / ch as f64)
        .collect()
}

fn db(x: f64) -> String {
    if x <= -240.0 {
        "−∞".to_string()
    } else {
        format!("{x:+.2} dB")
    }
}

/// 线性幅度 → dBFS。`0` 取不出对数（会是 −∞），按 `db()` 的口径折成 −240。
fn lin_db(x: f64) -> f64 {
    if x > 0.0 {
        20.0 * x.log10()
    } else {
        -240.0
    }
}

// ── check ──────────────────────────────────────────────────────

fn cmd_check(rest: &[String]) -> Result<(), Fail> {
    let a = parse(rest, &["--rate", "--fft"])?;
    a.extra_pos(1)?;
    let path = a.need_pos("<config.json>", 0)?;
    let rate = a.num("--rate", 48000.0)?;
    let fft_len = a.uint("--fft", 32768)? as usize;

    let cfg = read_config(path, rate)?;
    let chain =
        Chain::build(&cfg, rate).map_err(|e| Fail::Run(format!("配置 {path} 编不成链：{e}")))?;

    println!("配置      {path} @ {rate:.0} Hz");
    println!("preamp    {}", db(cfg.preamp_db));
    println!("滤波器    {} 段", cfg.filters.len());
    for (i, f) in cfg.filters.iter().enumerate() {
        println!(
            "  {:>2}  {:<11} {:>9.1} Hz   {:>9}   Q {:.3}",
            i + 1,
            f.kind.name(),
            f.freq_hz,
            if f.kind.uses_gain() {
                db(f.gain_db)
            } else {
                "—".to_string()
            },
            f.q
        );
    }

    // 卷积用的 IR（配了才打印）。读盘 / 校验已在 `Chain::build` 里做过 —— 能走到这里就说明
    // 那条 IR 是单声道且采样率一致。
    if let Some(p) = &cfg.convolution {
        println!("卷积      {p}（{} 抽头）", chain.ir().map_or(0, |ir| ir.len()));
    }

    // 实测 vs 理论：把 `measure` 那条 ±0.1dB 的口径在这里跑一遍，一条命令就能验完。
    let r = chain
        .response(rate, fft_len)
        .map_err(|e| Fail::Run(format!("对拍失败：{e}")))?;
    let mut comparable = 0usize;
    let mut worst = 0.0f64;
    let mut worst_at = 0.0f64;
    for (i, &f) in r.freqs.iter().enumerate() {
        let theory = chain.response_db(f, rate);
        if theory > -60.0 {
            comparable += 1;
            let d = (r.db[i] - theory).abs();
            if d > worst {
                worst = d;
                worst_at = f;
            }
        }
    }
    let verdict = if worst <= 0.1 { "通过" } else { "未通过" };
    println!(
        "对拍      {fft_len} 点冲激响应：最大偏差 {worst:.3} dB @ {worst_at:.1} Hz（{verdict}，\
         可比点 {comparable}/{}）",
        r.freqs.len()
    );
    println!("尾部      {}（越低说明这次测量越可信）", db(r.tail_db));

    if worst > 0.1 {
        return Err(Fail::Run(format!(
            "实测与理论的最大偏差 {worst:.3} dB 超过了 0.1 dB（尾部 {}）。\
             尾部高于 −100dB 时先加大 --fft（缓冲区不够，截断会造出假波纹）",
            db(r.tail_db)
        )));
    }
    Ok(())
}

// ── measure ────────────────────────────────────────────────────

fn cmd_measure(rest: &[String]) -> Result<(), Fail> {
    let a = parse(rest, &["--ref"])?;
    a.extra_pos(1)?;
    let path = a.need_pos("<wav>", 0)?;

    let audio = read_wav(path)?;
    let t = measure::tone_level(&mono(&audio), audio.sample_rate as f64)
        .map_err(|e| Fail::Run(format!("量不了 {path}：{e}")))?;

    println!("音频      {path}");
    println!(
        "          {} Hz / {} 声道 / {} 帧 / 峰值 {:.4} / 越界样本 {}",
        audio.sample_rate,
        audio.channels,
        audio.frames(),
        audio.peak(),
        audio.clipped_samples()
    );
    println!("主频      {:.2} Hz", t.freq_hz);
    println!("电平      {}", db(t.db));
    println!("本底      {}（离主频越远说明这段越像一个干净的单音）", db(t.floor_db));

    if let Some(ref_path) = a.get("--ref") {
        let ra = read_wav(ref_path)?;
        let rt = measure::tone_level(&mono(&ra), ra.sample_rate as f64)
            .map_err(|e| Fail::Run(format!("量不了参考 {ref_path}：{e}")))?;
        if (rt.freq_hz - t.freq_hz).abs() > 1.0 {
            return Err(Fail::Run(format!(
                "两次的主频差得太多（{} Hz vs {} Hz）—— 这不是同一段信号，比不出增益",
                rt.freq_hz, t.freq_hz
            )));
        }
        println!("参考      {ref_path}：主频 {:.2} Hz，电平 {}", rt.freq_hz, db(rt.db));
        println!("相对增益  {}", db(t.db - rt.db));
    }
    Ok(())
}

// ── gen ────────────────────────────────────────────────────────

fn cmd_gen(rest: &[String]) -> Result<(), Fail> {
    let flags = [
        "--kind",
        "--rate",
        "--freq",
        "--seconds",
        "--amp",
        "--channels",
        "-o",
    ];
    let a = parse(rest, &flags)?;
    a.extra_pos(0)?;
    let out = a
        .get("-o")
        .ok_or_else(|| Fail::Usage("gen 需要 -o <out.wav>".to_string()))?;

    let rate = a.num("--rate", 48000.0)?;
    let seconds = a.num("--seconds", 1.0)?;
    let channels = a.uint("--channels", 1)?;
    let kind = a.get("--kind").unwrap_or("tone");

    if !(1..=65535).contains(&channels) {
        return Err(Fail::Usage(format!("--channels 要在 1..65535，收到 {channels}")));
    }
    let rate = rate as u32;
    let channels = channels as u16;

    let audio = match kind {
        "tone" => gen::tone(
            rate,
            channels,
            a.num("--freq", 1000.0)?,
            seconds,
            a.num("--amp", 0.5)?,
        ),
        "silence" => gen::silence(rate, channels, seconds),
        other => {
            return Err(Fail::Usage(format!(
                "未知的 --kind {other:?}（可用：tone / silence）"
            )))
        }
    }
    .map_err(Fail::Run)?;

    write_wav(out, &audio)?;
    println!(
        "已写出    {out}：{} Hz / {} 声道 / {} 帧 / 峰值 {:.4}（{}）",
        audio.sample_rate,
        audio.channels,
        audio.frames(),
        audio.peak(),
        db(lin_db(audio.peak()))
    );
    Ok(())
}

// ── render ─────────────────────────────────────────────────────

fn cmd_render(rest: &[String]) -> Result<(), Fail> {
    let a = parse(rest, &["-c", "-i", "-o"])?;
    a.extra_pos(0)?;
    let cfg_path = a
        .get("-c")
        .ok_or_else(|| Fail::Usage("render 需要 -c <config.json>".to_string()))?;
    let in_path = a
        .get("-i")
        .ok_or_else(|| Fail::Usage("render 需要 -i <in.wav>".to_string()))?;
    let out_path = a
        .get("-o")
        .ok_or_else(|| Fail::Usage("render 需要 -o <out.wav>".to_string()))?;

    let audio = read_wav(in_path)?;
    // 配置里没有采样率 ⇒ 用**音频自己的**那个去校验与设计（理由见 `config` 模块头）。
    let rate = audio.sample_rate as f64;
    let cfg = read_config(cfg_path, rate)?;
    let out = chain::render(&audio, &cfg).map_err(|e| Fail::Run(format!("渲染失败：{e}")))?;
    write_wav(out_path, &out)?;

    println!("已写出    {out_path}：{} 帧", out.frames());
    println!("峰值      {:.4}（{}）", out.peak(), db(lin_db(out.peak())));
    if out.clipped_samples() > 0 {
        println!(
            "越界      {} 个样本会被钳位 —— 该减 preamp 了（写整数 WAV 时钳位是悄悄改内容）",
            out.clipped_samples()
        );
    } else {
        println!("越界      0（写出去不会被钳位）");
    }

    // 主频处的链增益（理论）—— 与 `measure --ref` 给的实测值正好是一对，方便直接比。
    let t = measure::tone_level(&mono(&audio), rate)
        .map_err(|e| Fail::Run(format!("量不了输入主频：{e}")))?;
    let c = Chain::build(&cfg, rate).map_err(|e| Fail::Run(format!("配置编不成链：{e}")))?;
    println!(
        "链增益    输入主频 {:.2} Hz 处 {}（理论，用 `measure --ref` 可对实测）",
        t.freq_hz,
        db(c.response_db(t.freq_hz, rate))
    );
    Ok(())
}
