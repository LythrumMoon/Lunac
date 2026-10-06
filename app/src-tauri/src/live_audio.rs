// 在线音乐（librespot）的音频接进调音链（2026-10-04）。
//
// 为什么有这一层：调音链（`tuning-engine` 的 `ChainRuntime`）原本只挂在**本地文件**那条
// 播放链上（见 `player.rs` 的 `TuningSource`）—— 在线歌单是 librespot 这个**独立进程**
// 自己解码、自己出声的，链一点都管不到。这个模块把 librespot 的输出改成**裸 PCM 走
// stdout**（`-B pipe`），再由我们读回来、套上**同一个** `TuningSource`、播到默认输出设备。
// 于是逐段开关 / 扬声器声道槽 / 延迟 / 声道复制 / 卷积 / If-Else **全部自动生效**，
// 因为它们本来就是挂在「输出链」上的，与音频从哪来无关。
//
// ── 三条已实测的事实（改这块之前先看这三条）───────────────────────────
// ① **stdout 只会有 PCM**：本机这份 0.8.0 用 `-B pipe -f F32` 启动，8 秒内 stdout
//    **0 字节**，而 stderr 里是 `Using StdoutSink (pipe) with format: F32` —— 日志走
//    **stderr**，所以 stdout 可以整条当成音频流。
// ② **例外是登录那一次**：`librespot-oauth` 的 `set_auth_url()` 无条件
//    `println!("Browse to: {auth_url}")`（**stdout**）。那行 ASCII 一旦混进 PCM，
//    轻则一声爆音，重则让后面的 f32 **整条错位 4 字节**（全是噪声）。所以
//    `music.rs` 的 `librespot_args` **只在非登录启动时**加 `-B pipe`（见那边的注释）。
// ③ **音量在 librespot 内部就已经乘过**（`player.rs:1749` 的
//    `volume_getter.attenuation_factor()` → 逐样本相乘，日志也写明 `softvol`）。
//    ⇒ 我们这条 Player 的音量**必须是 1.0**，否则 Spotify 那个音量条会被衰减两次。
//
// ── 音频线程纪律（硬）─────────────────────────────────────────────────
// rodio 的音频回调**不能阻塞、不能加锁**：`TuningSource` 是逐样本被拉的，所以这里
// **不读管道、不加 Mutex** —— reader 线程把数据搬进一个**无锁 SPSC 环形缓冲**，
// 音频线程只做一次原子读。缓冲空了就吐静音（时间轴不能断），绝不 `read()`。

use std::io::Read;
use std::num::{NonZeroU16, NonZeroU32};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rodio::{ChannelCount, Sample, SampleRate, Source};

/// 环形缓冲容量（**样本数**，含 L/R 交错）—— 1 秒。
///
/// 它同时是「抗卡顿的上限」和「ring 侧延迟的上限」；实际水位见 `RING_TARGET`。
const RING_CAP: usize = 44_100 * 2;

/// reader 线程维持的目标水位（样本数）≈ 125ms。
///
/// **不能贪多**：稳态延迟 = 本水位 + 管道缓冲。librespot 的生产速度不是问题（它写不下
/// 就会阻塞 —— 这正是我们要的背压：读得慢，它就少解码），所以水位只是「reader 线程
/// 被调度延迟多久也不至于断音」的余量。Windows 的 `thread::sleep` 粒度约 15ms，
/// 125ms 有足够余量，又不会让暂停/切歌手感发木。
const RING_TARGET: usize = 44_100 * 2 / 8;

/// Spotify 的流一律 **44.1kHz / 2 声道**（Ogg Vorbis，96/160/320 三个档都是）。
///
/// pipe 后端写的是**裸样本**、不带任何头，所以这两个数只能在这里声明 —— 上游若哪天
/// 改了规格，表现是「音调/声道不对」，改这里即可（`AudioPacket` 也不带这两个信息）。
const LIBRESPOT_CHANNELS: u16 = 2;
const LIBRESPOT_SAMPLE_RATE: u32 = 44_100;

/// 无锁 SPSC 环形缓冲（生产者 = reader 线程，消费者 = 音频线程）。
///
/// **刻意不用 `Mutex<VecDeque<f32>>`**：那等于在音频回调里逐个样本加锁 ——
/// `player.rs` 的文件头把这条列为硬纪律（优先级反转）。
/// 也**不用 `unsafe`**：槽位是 `AtomicU32` 存 f32 的位模式，代价是一次宽松原子读写
/// （只在音频线程可接受），换来的是「没有任何未定义行为可谈」。
struct Ring {
    buf: Box<[AtomicU32]>,
    /// 消费位（音频线程推进）。
    head: AtomicUsize,
    /// 生产位（reader 线程推进）。
    tail: AtomicUsize,
}

impl Ring {
    fn new(cap: usize) -> Self {
        let buf: Vec<AtomicU32> = (0..cap).map(|_| AtomicU32::new(0)).collect();
        Self {
            buf: buf.into_boxed_slice(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    fn len(&self) -> usize {
        self.tail
            .load(Ordering::Acquire)
            .wrapping_sub(self.head.load(Ordering::Acquire))
    }

    /// 生产一个样本；满了返回 `false`（**不阻塞、不覆盖** —— 覆盖等于丢音频）。
    fn push(&self, v: f32) -> bool {
        let tail = self.tail.load(Ordering::Relaxed);
        if tail.wrapping_sub(self.head.load(Ordering::Acquire)) >= self.buf.len() {
            return false;
        }
        self.buf[tail % self.buf.len()].store(v.to_bits(), Ordering::Relaxed);
        // Release：保证「槽位已写入」对消费者的 Acquire 读可见（head 的读在 len 里）。
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        true
    }

    fn pop(&self) -> Option<f32> {
        let head = self.head.load(Ordering::Relaxed);
        if head == self.tail.load(Ordering::Acquire) {
            return None;
        }
        let v = f32::from_bits(self.buf[head % self.buf.len()].load(Ordering::Relaxed));
        self.head.store(head.wrapping_add(1), Ordering::Release);
        Some(v)
    }
}

/// reader 线程：把管道里的字节攒成 f32 喂进 ring。
///
/// 两个「不做就会出错」的细节：
/// ① **字节可能半路断开**（管道写满时写方只写进去一部分），所以先用 `pending` 攒够
///    4 字节才吐一个样本 —— 直接 `read_exact` 一个 f32 会在边界处丢样本。
/// ② **水位到了就不读**（`RING_TARGET`）：读光了 librespot 就会一直解码、一直往管道里
///    灌，延迟会涨到「ring 满 + 管道满」；不读才是我们控制延迟的手段（写方阻塞 = 背压）。
fn reader_loop(
    mut r: impl Read,
    ring: Arc<Ring>,
    stop: Arc<AtomicBool>,
    label: &'static str,
) {
    let mut chunk = [0u8; 16 * 1024];
    // 攒字节用的缓冲 + 游标（用游标而不是 `drain(..4)`：后者每吐一个样本都要搬一次内存）。
    let mut pending: Vec<u8> = Vec::with_capacity(32 * 1024);
    let mut pos = 0usize;

    'outer: loop {
        if stop.load(Ordering::Acquire) {
            break;
        }
        // 水位够了就先别读（背压，见上）。
        if ring.len() >= RING_TARGET {
            std::thread::sleep(Duration::from_millis(4));
            continue;
        }
        match r.read(&mut chunk) {
            // EOF：librespot 退出了（正常关闭 / 被 kill / 崩溃）。
            Ok(0) => break,
            Ok(n) => pending.extend_from_slice(&chunk[..n]),
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                crate::log::warn(format!("librespot: 读取输出流失败（{label}）：{e}"));
                break;
            }
        }
        while pending.len() - pos >= 4 {
            let v = f32::from_le_bytes([
                pending[pos],
                pending[pos + 1],
                pending[pos + 2],
                pending[pos + 3],
            ]);
            pos += 4;
            // ring 满 = 音频线程还没消费，等它（这里等是安全的：不在音频线程上）。
            while !ring.push(v) {
                if stop.load(Ordering::Acquire) {
                    break 'outer;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        // 回收已消费的前缀（攒到 8KB 再搬一次，避免高频 memmove）。
        if pos == pending.len() {
            pending.clear();
            pos = 0;
        } else if pos >= 8192 {
            pending.drain(..pos);
            pos = 0;
        }
    }
    crate::log::info(format!("librespot: {label} 输出流已结束"));
}

/// 把 ring 当成一条无限长的 rodio 源。
struct PipeSource {
    ring: Arc<Ring>,
    channels: ChannelCount,
    rate: SampleRate,
}

impl Iterator for PipeSource {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        // **音频线程**：绝不阻塞、绝不加锁。空了就吐静音 —— 返回 `None` 会让这条源
        // 就此结束（暂停一下就永久没声），而吐静音只是那几毫秒没声音。
        Some(self.ring.pop().unwrap_or(0.0))
    }
}

impl Source for PipeSource {
    /// `None` = 无限长（本模块的 `next()` 也永远不返回 `None`）。
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> ChannelCount {
        self.channels
    }
    fn sample_rate(&self) -> SampleRate {
        self.rate
    }
    /// 直播流，没有总时长（进度条归 Spotify 那条远端状态管）。
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

/// 一条正在跑的在线播放链。
///
/// `sink` 与 `player` **只为「活着」而存在**（没有任何地方读它们）：rodio 的规矩是
/// `DeviceSink` 一 drop 输出流就没了 —— 所以它们必须被持有到 `stop()` 那一刻。
/// `stop` 则是给 reader 线程的退出信号。
#[allow(dead_code)]
struct LiveSession {
    sink: rodio::MixerDeviceSink,
    player: rodio::Player,
    stop: Arc<AtomicBool>,
}

static LIVE: Mutex<Option<LiveSession>> = Mutex::new(None);

/// 把 librespot 的 stdout 接上：读 PCM → 调音链 → 默认输出设备。
///
/// 只收一个 `reader`（= 子进程的 stdout）：声道数与采样率是本模块的常量
/// （`LIBRESPOT_CHANNELS` / `LIBRESPOT_SAMPLE_RATE`，pipe 后端不带任何头，只能在这里声明）。
/// 调用方在 `music.rs` 里**只传 stdout**，stderr 仍旧走日志。
pub fn start(reader: impl Read + Send + 'static) -> Result<(), String> {
    // 重起 librespot 时上一条可能还在（它只是没数据可放），先收干净。
    stop();
    let mut sink = rodio::DeviceSinkBuilder::open_default_sink()
        .map_err(|_| "ERR_NO_AUDIO_DEVICE".to_string())?;
    // 与 `player.rs` 同一个理由：默认会在 drop 时往 stderr 打一句噪音。
    sink.log_on_drop(false);
    let player = rodio::Player::connect_new(sink.mixer());
    // **1.0**：音量已经在 librespot 里按 Connect 的音量乘过了（见文件头 ③）。
    player.set_volume(1.0);
    let ring = Arc::new(Ring::new(RING_CAP));
    let stop = Arc::new(AtomicBool::new(false));
    player.append(crate::player::TuningSource::new(
        PipeSource {
            ring: ring.clone(),
            channels: NonZeroU16::new(LIBRESPOT_CHANNELS).expect("声道数常量非 0"),
            rate: NonZeroU32::new(LIBRESPOT_SAMPLE_RATE).expect("采样率常量非 0"),
        },
        crate::player::tuning_shared(),
    ));
    {
        let ring = ring.clone();
        let stop = stop.clone();
        std::thread::spawn(move || reader_loop(reader, ring, stop, "在线音乐"));
    }
    *LIVE.lock().map_err(|e| e.to_string())? = Some(LiveSession { sink, player, stop });
    Ok(())
}

/// 收掉在线播放链（**幂等**：没在跑时是空操作）。
///
/// 只置 `stop` 标志并 drop sink / player：reader 线程卡在 `read()` 上，得等 librespot
/// 的 stdout 关闭（调用方杀进程）才会退出 —— 顺序就是「先杀进程，再调这里」。
pub fn stop() {
    if let Ok(mut slot) = LIVE.lock() {
        if let Some(s) = slot.take() {
            s.stop.store(true, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_wraps_and_keeps_order() {
        let r = Ring::new(4);
        assert_eq!(r.len(), 0);
        assert_eq!(r.pop(), None);
        for v in [1.0f32, 2.0, 3.0, 4.0] {
            assert!(r.push(v));
        }
        // 满了就不再收（不覆盖、不丢老数据）
        assert!(!r.push(5.0));
        assert_eq!(r.len(), 4);
        // 绕圈：消费两个再放两个，读回来必须还是先进先出
        assert_eq!(r.pop(), Some(1.0));
        assert_eq!(r.pop(), Some(2.0));
        assert!(r.push(5.0));
        assert!(r.push(6.0));
        let got: Vec<f32> = std::iter::from_fn(|| r.pop()).collect();
        assert_eq!(got, vec![3.0, 4.0, 5.0, 6.0]);
        assert_eq!(r.pop(), None);
    }

    /// 空 ring 必须吐静音（而不是 `None`）—— 吐 `None` 会让这条源结束，
    /// 表现是「暂停一下再播就永久没声了」。
    #[test]
    fn an_empty_pipe_source_yields_silence_instead_of_ending() {
        let mut s = PipeSource {
            ring: Arc::new(Ring::new(8)),
            channels: NonZeroU16::new(LIBRESPOT_CHANNELS).unwrap(),
            rate: NonZeroU32::new(LIBRESPOT_SAMPLE_RATE).unwrap(),
        };
        assert_eq!(s.next(), Some(0.0));
        s.ring.push(0.25);
        assert_eq!(s.next(), Some(0.25));
        assert_eq!(s.next(), Some(0.0));
        // 无限长：rodio 不会因为 span 为 0 就把它当成放完了
        assert!(!s.is_exhausted());
        assert_eq!(s.sample_rate().get(), 44_100);
        assert_eq!(s.channels().get(), 2);
    }
}
