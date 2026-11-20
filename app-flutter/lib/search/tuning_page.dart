// 调音页（v1 音乐插件第三段 `#music-v-tuning` 的原生等价物）。
//
// 对齐依据：`docs/archive/ai-spec.md` §4.6「调音」那一节（Peace 三栏骨架 + 频段柱 +
// 通道槽 + 全局效果器 + A-B 盲测 + 频响曲线独立窗）。
//
// 边界（与 `platform/tuning.dart` 的分工）：
//   · 链的合法性、曲线、预设文件、A/B 快照全在 Rust（`rust/src/tuning.rs` +
//     `tuning-engine/`）—— 这里**不算任何滤波器、不算任何曲线**；
//   · 这里只维护「草稿」：改了哪一格就先改草稿，再整条提交（宿主语义是覆盖整条链）；
//   · 提交失败**回滚草稿**（重新读宿主真值），不留下一个「看着生效、其实没落盘」的界面。
//
// 三栏（v1 按 Peace 主窗量出来的结构）：
//   左 180px  = 保存 / 导入 / 导出 + 我的预设 + A-B 盲测
//   中 自适应 = 总增益 / 低音 / 高音三条滑块 + 频段柱（作用域 = 右栏选中的通道槽）
//   右 220px  = 扬声器声道槽 + 全局效果器（延迟 / 声道复制 / 卷积）
//
// 两条 v1 的纪律照搬：
//   ① 三条滑块与频段柱「按下即生效、即落盘」—— 与音质那两条「攒着等保存」刻意不同；
//   ② 全局效果器（延迟 / 复制 / 卷积）只在「全局」槽显示 —— 它们是引擎顶层字段，
//      画在通道槽里会看起来像只作用于那条声道。
import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../platform/tuning.dart';
import '../theme.dart';

/// 曲线窗的可见窗口（对数频率）：`zoom = 1` 是整条轴；放大后以 `center`（0..1）
/// 为中点，并夹在整条轴之内。
///
/// 抽成纯函数是为了单测 —— 滚轮事件本身在无头环境里没法可靠模拟，但「窗口怎么算、
/// 边界怎么夹」是这块最容易出错的数学。
({double lo, double hi}) zoomWindow({
  required double zoom,
  required double center,
  required double lo,
  required double hi,
}) {
  final full = hi - lo;
  final width = full / math.max(1.0, zoom);
  final half = width / 2;
  final mid = (lo + center.clamp(0.0, 1.0) * full).clamp(lo + half, hi - half);
  return (lo: mid - half, hi: mid + half);
}

/// 声道名（与 v1 `CHANNEL_NAMES` 同序：Peace / 7.1 那套）。
const List<String> _channelNames = ['L', 'R', 'C', 'LFE', 'SL', 'SR', 'BL', 'BR'];

/// 引擎类型名 → 中文（v1 `music.tk_*`）。
const Map<String, String> _kindLabels = {
  'peaking': '峰值',
  'low_shelf': '低架',
  'high_shelf': '高架',
  'low_pass': '低通',
  'high_pass': '高通',
  'band_pass': '带通',
  'notch': '陷波',
  'all_pass': '全通',
  'low_pass_1': '低通（一阶）',
  'high_pass_1': '高通（一阶）',
};

/// 与引擎 `MAX_DELAY_MS` 必须一致（超范围会让整条链落不下去）。
const double _maxDelayMs = 5000;
/// 增益收在 ±24dB（引擎的判据）；频率收在 20–20kHz（远低于任何常见 Nyquist）。
const double _maxGainDb = 24;

String _kindLabel(String name) => _kindLabels[name] ?? name;

String _channelName(int index) =>
    index >= 0 && index < _channelNames.length ? _channelNames[index] : '#${index + 1}';

/// 数字的显示形态：最多 `digits` 位小数，尾随的 0 去掉（v1 `fmtTuneNum` 同口径）。
String _fmt(double value, int digits) {
  if (!value.isFinite) {
    return '0';
  }
  var text = value.toStringAsFixed(digits);
  if (text.contains('.')) {
    text = text.replaceFirst(RegExp(r'0+$'), '');
    text = text.replaceFirst(RegExp(r'\.$'), '');
  }
  return text;
}

double _clampFreq(double value) =>
    value.isFinite ? value.clamp(20.0, 20000.0) : 1000;
double _clampGain(double value) =>
    value.isFinite ? value.clamp(-_maxGainDb, _maxGainDb) : 0;

/// 调音页。
class TuningPage extends StatefulWidget {
  const TuningPage({super.key, required this.onClose});

  /// 返回（v1 是「标题栏那个 ←」）。
  final VoidCallback onClose;

  @override
  State<TuningPage> createState() => _TuningPageState();
}

class _TuningPageState extends State<TuningPage> {
  TuningState? _state;

  /// 每个通道槽的草稿：`[0]` = 全局（引擎的 `filters`），`[k]` = 声道 `k-1` 的 then 分支。
  List<List<TuningFilter>> _slots = [<TuningFilter>[]];
  int _slot = 0;

  double _preamp = 0;
  double _bass = 0;
  double _treble = 0;
  double _delay = 0;
  List<TuningCopy> _copy = [];
  String? _conv;

  String? _notice;
  String? _error;

  Timer? _poll;
  Timer? _debounce;

  ({bool a, bool b}) _abHas = (a: false, b: false);
  List<String> _abSlots = ['a', 'b'];
  bool _blind = false;
  bool _revealed = false;
  String? _abActive;

  /// 宿主真值的签名：轮询时只在它变了才重铺草稿（否则会打断正在输的字）。
  String _signature = '';

  @override
  void initState() {
    super.initState();
    _load();
    _abHas = tuningAbState();
    // v1 的「反向同步」：曲线窗 / A-B 改了链，这页跟着动（1.2s 轻轮询）。
    _poll = Timer.periodic(
      const Duration(milliseconds: 1200),
      (_) => _pollHost(),
    );
  }

  @override
  void dispose() {
    _poll?.cancel();
    _debounce?.cancel();
    super.dispose();
  }

  // ── 读 / 写 ─────────────────────────────────────────────────────

  String _sigOf(TuningState s) => [
    s.enabled,
    s.preset,
    s.preampDb,
    s.bassDb,
    s.trebleDb,
    s.delayMs,
    s.convolution ?? '',
    s.channels,
    for (final f in s.filters) '${f.kind}/${f.freqHz}/${f.gainDb}/${f.q}/${f.on}',
    for (final c in s.channelCopy) '${c.from}>${c.to}',
    for (final b in s.ifElse)
      '${b.channel}:${b.thenFilters.length}/${b.elseFilters.length}',
    for (final d in s.response.db) d.toStringAsFixed(2),
  ].join('|');

  void _load() {
    final reply = readTuning();
    if (!reply.ok || reply.state == null) {
      setState(() {
        _error = reply.error.isEmpty ? '读不到调音状态' : reply.error;
      });
      return;
    }
    setState(() => _apply(reply.state!));
  }

  void _pollHost() {
    final reply = readTuning();
    final state = reply.state;
    if (!reply.ok || state == null) {
      return;
    }
    if (_sigOf(state) == _signature) {
      return;
    }
    if (!mounted) {
      return;
    }
    setState(() => _apply(state));
  }

  /// 把宿主真值铺成草稿（v1 `syncTuningDraft`）。
  void _apply(TuningState state) {
    _state = state;
    _signature = _sigOf(state);
    final channels = state.channels > 0 ? state.channels : 2;
    final slots = <List<TuningFilter>>[List<TuningFilter>.of(state.filters)];
    for (var k = 1; k <= channels; k++) {
      final block = state.ifElse
          .where((b) => b.channel == k - 1)
          .toList(growable: false);
      slots.add(
        block.isEmpty ? <TuningFilter>[] : List<TuningFilter>.of(block.first.thenFilters),
      );
    }
    _slots = slots;
    if (_slot < 0 || _slot >= slots.length) {
      _slot = 0;
    }
    _preamp = state.preampDb;
    _bass = state.bassDb;
    _treble = state.trebleDb;
    _delay = state.delayMs;
    _copy = List<TuningCopy>.of(state.channelCopy);
    _conv = state.convolution;
  }

  /// 把全部通道槽草稿拼成宿主的 `if_else`。
  ///
  /// `else` 分支面板没有界面 ⇒ **原样保留**宿主那一份（不回传等于替用户把半边删了）。
  List<TuningCondBlock> _ifElsePayload() {
    final previous = {
      for (final block in _state?.ifElse ?? const <TuningCondBlock>[])
        block.channel: block,
    };
    final out = <TuningCondBlock>[];
    for (var k = 1; k < _slots.length; k++) {
      final channel = k - 1;
      final then = _slots[k];
      final els = previous[channel]?.elseFilters ?? const <TuningFilter>[];
      if (then.isEmpty && els.isEmpty) {
        continue;
      }
      out.add(
        TuningCondBlock(channel: channel, thenFilters: then, elseFilters: els),
      );
    }
    return out;
  }

  /// 提交整条链。`quiet` = 拖动中的节流提交（不每次喊「已保存」）。
  void _commit({bool quiet = false}) {
    final reply = setTuningChain(
      preampDb: _preamp,
      bassDb: _bass,
      trebleDb: _treble,
      filters: _slots[0],
      graphicEq: _state?.graphicEq ?? const <Object?>[],
      delayMs: _delay,
      channelCopy: _copy,
      convolution: _conv,
      ifElse: _ifElsePayload(),
    );
    if (!mounted) {
      return;
    }
    if (!reply.ok) {
      // 校验不过就**回滚界面**（链的合法性只由引擎兜底）。
      final message = reply.error;
      _load();
      setState(() {
        _error = message.isEmpty ? '保存失败' : message;
        _notice = null;
      });
      return;
    }
    final state = reply.state;
    setState(() {
      if (state != null) {
        _apply(state);
      }
      _error = null;
      if (!quiet) {
        _notice = '已保存';
      }
    });
  }

  void _scheduleCommit() {
    _debounce?.cancel();
    _debounce = Timer(const Duration(milliseconds: 120), () => _commit(quiet: true));
  }

  void _setEnabled(bool enabled) {
    final reply = setTuningEnabled(enabled);
    if (!mounted) {
      return;
    }
    if (!reply.ok) {
      setState(() => _error = reply.error.isEmpty ? '切换失败' : reply.error);
      return;
    }
    final state = reply.state;
    setState(() {
      if (state != null) {
        _apply(state);
      }
      _error = null;
    });
  }

  // ── 界面 ───────────────────────────────────────────────────────

  @override
  Widget build(BuildContext context) {
    final state = _state;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _buildHeader(state),
        if (state != null)
          Expanded(
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                SizedBox(width: 180, child: _buildLeftColumn(state)),
                const VerticalDivider(width: 1),
                Expanded(child: _buildCenterColumn(state)),
                const VerticalDivider(width: 1),
                SizedBox(width: 220, child: _buildRightColumn(state)),
              ],
            ),
          )
        else
          Expanded(
            child: Center(
              child: Text(
                _error ?? '正在读取调音状态…',
                style: TextStyle(color: LunacColors.textMuted, fontSize: 12),
              ),
            ),
          ),
        _buildStatusLine(state),
      ],
    );
  }

  Widget _buildHeader(TuningState? state) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(10, 8, 12, 6),
      child: Row(
        children: [
          IconButton(
            tooltip: '返回（Esc）',
            icon: const Icon(Icons.arrow_back, size: 18),
            color: LunacColors.textDim,
            visualDensity: VisualDensity.compact,
            onPressed: widget.onClose,
          ),
          const SizedBox(width: 2),
          Text(
            '调音',
            style: TextStyle(
              color: LunacColors.text,
              fontSize: 14,
              fontWeight: FontWeight.w600,
            ),
          ),
          const SizedBox(width: 8),
          if (state != null)
            Text(
              '采样率 ${_fmt(state.response.sampleRate / 1000, 1)}kHz · '
              '${state.channels > 0 ? state.channels : 2} 声道',
              style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
            ),
          const Spacer(),
          TextButton.icon(
            onPressed: state == null ? null : _openCurve,
            icon: const Icon(Icons.show_chart, size: 13),
            label: const Text('频响曲线…', style: TextStyle(fontSize: 10.5)),
            style: TextButton.styleFrom(
              foregroundColor: LunacColors.textDim,
              padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
              minimumSize: Size.zero,
              visualDensity: VisualDensity.compact,
            ),
          ),
          const SizedBox(width: 8),
          Tooltip(
            message: state?.enabled == true ? '关闭调音' : '打开调音',
            child: Switch(
              value: state?.enabled == true,
              onChanged: state == null ? null : _setEnabled,
              materialTapTargetSize: MaterialTapTargetSize.shrinkWrap,
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildStatusLine(TuningState? state) {
    final text = _error ?? _notice;
    return SizedBox(
      height: 20,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 12),
        child: Align(
          alignment: Alignment.centerLeft,
          child: Text(
            text ??
                (state?.enabled == false
                    ? '调音关着：链是逐位直通的（打开后才会改变声音）'
                    : ''),
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(
              color: _error != null ? LunacColors.danger : LunacColors.textMuted,
              fontSize: 11,
            ),
          ),
        ),
      ),
    );
  }

  // ── 左栏：预设 + A-B ────────────────────────────────────────────

  Widget _buildLeftColumn(TuningState state) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(10, 4, 10, 4),
          child: Wrap(
            spacing: 6,
            runSpacing: 4,
            children: [
              _smallButton('保存', _savePresetDialog),
              _smallButton('导入', _importPreset),
              _smallButton('导出', _exportPreset),
            ],
          ),
        ),
        Padding(
          padding: const EdgeInsets.fromLTRB(10, 6, 10, 2),
          child: Text(
            '我的预设',
            style: TextStyle(color: LunacColors.textMuted, fontSize: 10.5),
          ),
        ),
        Expanded(
          child: state.userPresets.isEmpty
              ? Padding(
                  padding: const EdgeInsets.symmetric(horizontal: 10),
                  child: Text(
                    '还没有自己的预设',
                    style: TextStyle(color: LunacColors.textDim, fontSize: 10.5),
                  ),
                )
              : ListView.builder(
                  padding: EdgeInsets.zero,
                  itemCount: state.userPresets.length,
                  itemBuilder: (context, index) {
                    final name = state.userPresets[index];
                    final active = name == state.preset;
                    return InkWell(
                      onTap: () => _applyPreset(name),
                      child: Container(
                        padding: const EdgeInsets.fromLTRB(10, 4, 4, 4),
                        color: active
                            ? LunacColors.accent.withValues(alpha: 0.18)
                            : null,
                        child: Row(
                          children: [
                            Expanded(
                              child: Text(
                                name,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: TextStyle(
                                  color: active
                                      ? LunacColors.accent
                                      : LunacColors.textDim,
                                  fontSize: 11.5,
                                ),
                              ),
                            ),
                            _presetMenu(name),
                          ],
                        ),
                      ),
                    );
                  },
                ),
        ),
        const Divider(height: 12),
        _buildAbBlock(),
      ],
    );
  }

  Widget _presetMenu(String name) {
    return PopupMenuButton<String>(
      tooltip: '更多',
      padding: EdgeInsets.zero,
      iconSize: 14,
      icon: Icon(Icons.more_horiz, size: 14, color: LunacColors.textMuted),
      onSelected: (value) {
        if (value == 'rename') {
          _renamePresetDialog(name);
        } else if (value == 'delete') {
          _deletePreset(name);
        }
      },
      itemBuilder: (context) => const [
        PopupMenuItem(value: 'rename', height: 30, child: Text('重命名', style: TextStyle(fontSize: 12))),
        PopupMenuItem(value: 'delete', height: 30, child: Text('删除', style: TextStyle(fontSize: 12))),
      ],
    );
  }

  Widget _buildAbBlock() {
    final pos1 = _abSlots[0];
    final pos2 = _abSlots[1];
    bool has(String slot) => slot == 'a' ? _abHas.a : _abHas.b;
    final both = _abHas.a && _abHas.b;
    return Padding(
      padding: const EdgeInsets.fromLTRB(10, 0, 10, 8),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            'A-B 盲测',
            style: TextStyle(color: LunacColors.textMuted, fontSize: 10.5),
          ),
          const SizedBox(height: 4),
          Row(
            children: [
              _smallButton('存到 A', () => _saveAb('a')),
              const SizedBox(width: 6),
              _smallButton('存到 B', () => _saveAb('b')),
            ],
          ),
          const SizedBox(height: 4),
          Wrap(
            spacing: 6,
            runSpacing: 4,
            children: [
              _abButton(
                label: _blind ? '1' : pos1.toUpperCase(),
                enabled: has(pos1),
                active: _abActive == pos1,
                onTap: () => _applyAb(pos1),
              ),
              _abButton(
                label: _blind ? '2' : pos2.toUpperCase(),
                enabled: has(pos2),
                active: _abActive == pos2,
                onTap: () => _applyAb(pos2),
              ),
              _abButton(
                label: '盲测',
                enabled: both,
                active: _blind,
                onTap: _toggleBlind,
              ),
              if (_blind && !_revealed)
                _abButton(
                  label: '揭晓',
                  enabled: true,
                  active: false,
                  onTap: () => setState(() => _revealed = true),
                ),
            ],
          ),
          const SizedBox(height: 4),
          Text(
            _abHint(),
            style: TextStyle(color: LunacColors.textDim, fontSize: 10),
          ),
        ],
      ),
    );
  }

  String _abHint() {
    if (!_abHas.a && !_abHas.b) {
      return '两个槽都还没有快照';
    }
    if (!_blind) {
      return 'A ${_abHas.a ? '有' : '—'} · B ${_abHas.b ? '有' : '—'}（两边都存了才能盲测）';
    }
    if (!_revealed) {
      return '盲测中：1 / 2 的对应关系是乱的，点「揭晓」看答案';
    }
    return '揭晓：1 = ${_abSlots[0].toUpperCase()} · 2 = ${_abSlots[1].toUpperCase()}';
  }

  Widget _abButton({
    required String label,
    required bool enabled,
    required bool active,
    required VoidCallback onTap,
  }) {
    return SizedBox(
      height: 24,
      child: TextButton(
        onPressed: enabled ? onTap : null,
        style: TextButton.styleFrom(
          padding: const EdgeInsets.symmetric(horizontal: 8),
          minimumSize: Size.zero,
          visualDensity: VisualDensity.compact,
          backgroundColor: active
              ? LunacColors.accent.withValues(alpha: 0.18)
              : null,
          foregroundColor: active ? LunacColors.accent : LunacColors.textDim,
        ),
        child: Text(label, style: const TextStyle(fontSize: 10.5)),
      ),
    );
  }

  Widget _smallButton(String label, VoidCallback onTap) {
    return SizedBox(
      height: 24,
      child: TextButton(
        onPressed: onTap,
        style: TextButton.styleFrom(
          padding: const EdgeInsets.symmetric(horizontal: 8),
          minimumSize: Size.zero,
          visualDensity: VisualDensity.compact,
          foregroundColor: LunacColors.textDim,
        ),
        child: Text(label, style: const TextStyle(fontSize: 10.5)),
      ),
    );
  }

  // ── 中栏：三条滑块 + 频段柱 ─────────────────────────────────────

  Widget _buildCenterColumn(TuningState state) {
    final filters = _slots[_slot];
    return Padding(
      padding: const EdgeInsets.fromLTRB(12, 6, 8, 4),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _sliderRow('总增益', _preamp, (v) {
            setState(() => _preamp = v);
            _scheduleCommit();
          }, () => _commit()),
          _sliderRow('低音增益', _bass, (v) {
            setState(() => _bass = v);
            _scheduleCommit();
          }, () => _commit()),
          _sliderRow('高音增益', _treble, (v) {
            setState(() => _treble = v);
            _scheduleCommit();
          }, () => _commit()),
          const SizedBox(height: 4),
          Expanded(
            child: filters.isEmpty
                ? Center(
                    child: Text(
                      '这一槽还没有频段 —— 点下面的「加一段」，或在曲线窗里点一下',
                      style: TextStyle(
                        color: LunacColors.textDim,
                        fontSize: 11,
                      ),
                    ),
                  )
                : SingleChildScrollView(
                    scrollDirection: Axis.horizontal,
                    child: Row(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        for (var i = 0; i < filters.length; i++)
                          _buildBand(state, filters, i),
                      ],
                    ),
                  ),
          ),
          const SizedBox(height: 4),
          Align(
            alignment: Alignment.centerLeft,
            child: SizedBox(
              height: 26,
              child: TextButton.icon(
                onPressed: _addBand,
                icon: const Icon(Icons.add, size: 14),
                label: const Text('加一段', style: TextStyle(fontSize: 10.5)),
                style: TextButton.styleFrom(
                  padding: const EdgeInsets.symmetric(horizontal: 8),
                  minimumSize: Size.zero,
                  visualDensity: VisualDensity.compact,
                  foregroundColor: LunacColors.textDim,
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _sliderRow(
    String label,
    double value,
    ValueChanged<double> onChanged,
    VoidCallback onDone,
  ) {
    return Row(
      children: [
        SizedBox(
          width: 56,
          child: Text(
            label,
            style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
          ),
        ),
        Expanded(
          child: Slider(
            min: -24,
            max: 24,
            divisions: 96,
            value: value.clamp(-24, 24),
            label: _fmt(value, 1),
            onChanged: onChanged,
            onChangeEnd: (_) => onDone(),
          ),
        ),
        SizedBox(
          width: 60,
          child: _NumField(
            value: value,
            decimals: 1,
            onSubmit: (v) {
              onChanged(_clampGain(v));
              onDone();
            },
          ),
        ),
      ],
    );
  }

  /// 一根频段柱（v1 `tuningBandHtml`）：频率（可编辑）/ 增益竖拉条 / 大号增益 / Q / 类型 + 开关 + 删除。
  Widget _buildBand(TuningState state, List<TuningFilter> filters, int index) {
    final filter = filters[index];
    final meta = state.kinds.where((k) => k.name == filter.kind).toList();
    final useGain = meta.isEmpty
        ? filter.gainDb != null
        : meta.first.usesGain;
    final useQ = meta.isEmpty ? filter.q != null : meta.first.order != 1;
    final gain = filter.gainDb ?? 0;
    final kindLabel = _kindLabel(filter.kind);
    return Container(
      width: 74,
      margin: const EdgeInsets.only(right: 6),
      decoration: BoxDecoration(
        border: Border.all(
          color: filter.on
              ? LunacColors.border
              : LunacColors.border.withValues(alpha: 0.4),
        ),
        borderRadius: BorderRadius.circular(6),
      ),
      padding: const EdgeInsets.fromLTRB(4, 4, 4, 2),
      child: Column(
        children: [
          _NumField(
            value: filter.freqHz,
            decimals: 1,
            width: 66,
            onSubmit: (v) => _editBand(
              index,
              (f) => f.copyWith(freqHz: _clampFreq(v)),
            ),
          ),
          Expanded(
            child: RotatedBox(
              quarterTurns: 3,
              child: Slider(
                min: -24,
                max: 24,
                divisions: 96,
                value: gain.clamp(-24, 24),
                onChanged: useGain
                    ? (v) => _editBand(
                        index,
                        (f) => f.copyWith(gainDb: _clampGain(v)),
                        commit: false,
                      )
                    : null,
                onChangeEnd: useGain ? (_) => _commit() : null,
              ),
            ),
          ),
          if (useGain)
            _NumField(
              value: gain,
              decimals: 1,
              width: 66,
              big: true,
              onSubmit: (v) => _editBand(
                index,
                (f) => f.copyWith(gainDb: _clampGain(v)),
              ),
            )
          else
            Text('—', style: TextStyle(color: LunacColors.textDim, fontSize: 12)),
          if (useQ)
            _NumField(
              value: filter.q ?? 0.707,
              decimals: 3,
              width: 66,
              onSubmit: (v) => _editBand(
                index,
                (f) => f.copyWith(q: v <= 0 ? 0.707 : v),
              ),
            )
          else
            const SizedBox(height: 18),
          const SizedBox(height: 2),
          PopupMenuButton<String>(
            tooltip: '类型',
            padding: EdgeInsets.zero,
            onSelected: (name) =>
                _editBand(index, (f) => f.copyWith(kind: name)),
            itemBuilder: (context) => [
              for (final kind in state.kinds)
                PopupMenuItem(
                  value: kind.name,
                  height: 30,
                  child: Text(
                    _kindLabel(kind.name),
                    style: const TextStyle(fontSize: 12),
                  ),
                ),
            ],
            child: Container(
              height: 20,
              alignment: Alignment.center,
              decoration: BoxDecoration(
                border: Border.all(color: LunacColors.border),
                borderRadius: BorderRadius.circular(4),
              ),
              child: Text(
                kindLabel,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(color: LunacColors.textDim, fontSize: 9.5),
              ),
            ),
          ),
          const SizedBox(height: 2),
          Row(
            mainAxisAlignment: MainAxisAlignment.spaceEvenly,
            children: [
              IconButton(
                tooltip: filter.on ? '关掉这一段' : '打开这一段',
                icon: Icon(
                  Icons.power_settings_new,
                  size: 13,
                  color: filter.on
                      ? LunacColors.accent
                      : LunacColors.textMuted,
                ),
                padding: EdgeInsets.zero,
                constraints: const BoxConstraints.tightFor(
                  width: 22,
                  height: 22,
                ),
                visualDensity: VisualDensity.compact,
                onPressed: () =>
                    _editBand(index, (f) => f.copyWith(on: !f.on)),
              ),
              IconButton(
                tooltip: '删除这一段',
                icon: Icon(Icons.delete_outline, size: 13, color: LunacColors.textMuted),
                padding: EdgeInsets.zero,
                constraints: const BoxConstraints.tightFor(
                  width: 22,
                  height: 22,
                ),
                visualDensity: VisualDensity.compact,
                onPressed: () => _removeBand(index),
              ),
            ],
          ),
        ],
      ),
    );
  }

  void _editBand(
    int index,
    TuningFilter Function(TuningFilter) change, {
    bool commit = true,
  }) {
    final filters = _slots[_slot];
    if (index < 0 || index >= filters.length) {
      return;
    }
    setState(() {
      filters[index] = change(filters[index]);
    });
    if (commit) {
      _commit();
    }
  }

  void _addBand() {
    final preset = _defaultKind(_state);
    setState(() {
      _slots[_slot].add(
        TuningFilter(
          kind: preset.name,
          freqHz: 1000,
          gainDb: preset.usesGain ? 0 : null,
          q: preset.order == 1 ? null : 0.707,
        ),
      );
    });
    _commit();
  }

  void _removeBand(int index) {
    final filters = _slots[_slot];
    if (index < 0 || index >= filters.length) {
      return;
    }
    setState(() => filters.removeAt(index));
    _commit();
  }

  TuningKind _defaultKind(TuningState? state) {
    final kinds = state?.kinds ?? const <TuningKind>[];
    for (final kind in kinds) {
      if (kind.name == 'peaking') {
        return kind;
      }
    }
    return kinds.isEmpty
        ? const TuningKind(name: 'peaking', usesGain: true, order: 2)
        : kinds.first;
  }

  // ── 右栏：声道槽 + 全局效果器 ───────────────────────────────────

  Widget _buildRightColumn(TuningState state) {
    final global = _slot == 0;
    return Padding(
      padding: const EdgeInsets.fromLTRB(8, 6, 10, 8),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            '扬声器声道',
            style: TextStyle(color: LunacColors.textMuted, fontSize: 10.5),
          ),
          const SizedBox(height: 4),
          for (var k = 0; k < _slots.length; k++)
            _slotRow(k, _slots[k].where((f) => f.on).length),
          const Divider(height: 14),
          if (!global)
            Text(
              '延迟 / 声道复制 / 卷积是整条链的设置，只在「全局」槽里显示',
              style: TextStyle(color: LunacColors.textDim, fontSize: 10),
            ),
          if (global) ...[
            _sliderRowRight('延迟', _delay, 0, _maxDelayMs, (v) {
              setState(() => _delay = v.clamp(0, _maxDelayMs));
              _scheduleCommit();
            }, () => _commit()),
            const SizedBox(height: 6),
            Text(
              '声道复制',
              style: TextStyle(color: LunacColors.textMuted, fontSize: 10.5),
            ),
            for (var i = 0; i < _copy.length; i++) _copyRow(state, i),
            Align(
              alignment: Alignment.centerLeft,
              child: _smallButton('加一条', _addCopy),
            ),
            const SizedBox(height: 6),
            Text(
              '卷积',
              style: TextStyle(color: LunacColors.textMuted, fontSize: 10.5),
            ),
            Text(
              _conv ?? '没启用',
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(color: LunacColors.textDim, fontSize: 10),
            ),
            Row(
              children: [
                _smallButton('选择…', _pickIr),
                if (_conv != null) _smallButton('清除', _clearIr),
              ],
            ),
          ],
        ],
      ),
    );
  }

  Widget _slotRow(int index, int enabledCount) {
    final active = index == _slot;
    final label = index == 0 ? '全局' : _channelName(index - 1);
    return InkWell(
      onTap: () => setState(() => _slot = index),
      child: Container(
        height: 26,
        padding: const EdgeInsets.symmetric(horizontal: 8),
        margin: const EdgeInsets.only(bottom: 2),
        decoration: BoxDecoration(
          color: active ? LunacColors.accent.withValues(alpha: 0.18) : null,
          borderRadius: BorderRadius.circular(4),
        ),
        child: Row(
          children: [
            Expanded(
              child: Text(
                label,
                style: TextStyle(
                  color: active ? LunacColors.accent : LunacColors.textDim,
                  fontSize: 11.5,
                ),
              ),
            ),
            Text(
              '$enabledCount',
              style: TextStyle(
                color: active ? LunacColors.accent : LunacColors.textMuted,
                fontSize: 10,
              ),
            ),
          ],
        ),
      ),
    );
  }

  /// 一条「左标签 + 滑块 + 右数值框」（右栏的延迟用；范围与中栏不同）。
  Widget _sliderRowRight(
    String label,
    double value,
    double min,
    double max,
    ValueChanged<double> onChanged,
    VoidCallback onDone,
  ) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            Expanded(
              child: Text(
                label,
                style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
              ),
            ),
            SizedBox(
              width: 60,
              child: _NumField(
                value: value,
                decimals: 1,
                onSubmit: (v) {
                  onChanged(v.clamp(min, max));
                  onDone();
                },
              ),
            ),
          ],
        ),
        Slider(
          min: min,
          max: max,
          divisions: ((max - min) / 10).round(),
          value: value.clamp(min, max),
          label: _fmt(value, 1),
          onChanged: onChanged,
          onChangeEnd: (_) => onDone(),
        ),
      ],
    );
  }

  Widget _copyRow(TuningState state, int index) {
    final copy = _copy[index];
    final channels = math.max(1, state.channels);
    return Padding(
      padding: const EdgeInsets.only(bottom: 2),
      child: Row(
        children: [
          _channelPicker(copy.from, channels, (v) {
            setState(() => _copy[index] = TuningCopy(from: v, to: _copy[index].to));
            _commit();
          }),
          Text(' → ', style: TextStyle(color: LunacColors.textMuted, fontSize: 11)),
          _channelPicker(copy.to, channels, (v) {
            setState(() => _copy[index] = TuningCopy(from: _copy[index].from, to: v));
            _commit();
          }),
          IconButton(
            tooltip: '删除',
            icon: Icon(Icons.close, size: 12, color: LunacColors.textMuted),
            padding: EdgeInsets.zero,
            constraints: const BoxConstraints.tightFor(width: 20, height: 20),
            visualDensity: VisualDensity.compact,
            onPressed: () {
              setState(() => _copy.removeAt(index));
              _commit();
            },
          ),
        ],
      ),
    );
  }

  Widget _channelPicker(int value, int channels, ValueChanged<int> onPick) {
    return PopupMenuButton<int>(
      tooltip: '选声道',
      padding: EdgeInsets.zero,
      onSelected: onPick,
      itemBuilder: (context) => [
        // 只让用户在**当前流的声道数以内**选：越界在引擎里是硬错误。
        for (var k = 0; k < channels; k++)
          PopupMenuItem(
            value: k,
            height: 30,
            child: Text(_channelName(k), style: const TextStyle(fontSize: 12)),
          ),
      ],
      child: Container(
        height: 20,
        padding: const EdgeInsets.symmetric(horizontal: 6),
        alignment: Alignment.center,
        decoration: BoxDecoration(
          border: Border.all(color: LunacColors.border),
          borderRadius: BorderRadius.circular(4),
        ),
        child: Text(
          _channelName(value),
          style: TextStyle(color: LunacColors.textDim, fontSize: 10),
        ),
      ),
    );
  }

  void _addCopy() {
    final channels = math.max(1, _state?.channels ?? 2);
    setState(() {
      _copy.add(TuningCopy(from: 0, to: channels > 1 ? 1 : 0));
    });
    _commit();
  }

  // ── 预设 / A-B / 卷积的动作 ─────────────────────────────────────

  Future<void> _savePresetDialog() async {
    final name = await _askName(title: '保存为预设', initial: _state?.preset ?? '');
    if (name == null || name.trim().isEmpty) {
      return;
    }
    var reply = saveTuningPreset(name.trim());
    if (!reply.ok && reply.error.contains('已存在')) {
      final overwrite = await _confirm('同名预设已存在', '要覆盖「$name」吗？');
      if (overwrite != true) {
        return;
      }
      reply = saveTuningPreset(name.trim(), overwrite: true);
    }
    if (!mounted) {
      return;
    }
    if (!reply.ok) {
      setState(() => _error = reply.error);
      return;
    }
    final state = reply.state;
    setState(() {
      if (state != null) {
        _apply(state);
      }
      _error = null;
      _notice = '已存为预设「${name.trim()}」';
    });
  }

  void _applyPreset(String name) {
    final reply = applyTuningPreset(name);
    if (!mounted) {
      return;
    }
    if (!reply.ok) {
      setState(() => _error = reply.error);
      return;
    }
    final state = reply.state;
    setState(() {
      if (state != null) {
        _apply(state);
      }
      _error = null;
      _notice = '已应用预设「$name」';
    });
  }

  Future<void> _renamePresetDialog(String name) async {
    final next = await _askName(title: '重命名预设', initial: name);
    if (next == null || next.trim().isEmpty || next.trim() == name) {
      return;
    }
    final reply = renameTuningPreset(name, next.trim());
    if (!mounted) {
      return;
    }
    if (!reply.ok) {
      setState(() => _error = reply.error);
      return;
    }
    final state = reply.state;
    setState(() {
      if (state != null) {
        _apply(state);
      }
      _error = null;
      _notice = '已改名';
    });
  }

  void _deletePreset(String name) {
    final reply = deleteTuningPreset(name);
    if (!mounted) {
      return;
    }
    if (!reply.ok) {
      setState(() => _error = reply.error);
      return;
    }
    final state = reply.state;
    setState(() {
      if (state != null) {
        _apply(state);
      }
      _error = null;
      _notice = '已删除预设「$name」';
    });
  }

  void _importPreset() {
    final path = pickTuningPresetFile();
    if (path == null) {
      return;
    }
    // **导入即应用**（与 v1 同口径）。
    final reply = importTuningPreset(path);
    if (!mounted) {
      return;
    }
    if (!reply.ok) {
      setState(() => _error = reply.error);
      return;
    }
    final state = reply.state;
    setState(() {
      if (state != null) {
        _apply(state);
      }
      _error = null;
      _notice = '已导入并应用';
    });
  }

  void _exportPreset() {
    final path = pickTuningExportPath();
    if (path == null) {
      return;
    }
    final reply = exportTuningPreset(path);
    if (!mounted) {
      return;
    }
    setState(() {
      _error = reply.ok ? null : reply.error;
      _notice = reply.ok ? '已导出到 $path' : null;
    });
  }

  void _saveAb(String slot) {
    final reply = saveTuningAb(slot);
    if (!mounted) {
      return;
    }
    setState(() {
      _error = reply.ok ? null : reply.error;
      _notice = reply.ok ? '已存到 ${slot.toUpperCase()}' : null;
    });
    _abHas = tuningAbState();
    if (!_abHas.a || !_abHas.b) {
      _blind = false;
      _revealed = false;
    }
  }

  void _applyAb(String slot) {
    final reply = applyTuningAb(slot);
    if (!mounted) {
      return;
    }
    if (!reply.ok) {
      setState(() => _error = reply.error);
      return;
    }
    final state = reply.state;
    setState(() {
      _abActive = slot;
      if (state != null) {
        _apply(state);
      }
      _error = null;
    });
  }

  void _toggleBlind() {
    if (!_abHas.a || !_abHas.b) {
      return;
    }
    setState(() {
      _blind = !_blind;
      _revealed = false;
      // 进盲测就先把映射打乱一次（否则第一次点开永远是「1 = A」）。
      if (_blind && math.Random().nextBool()) {
        _abSlots = [_abSlots[1], _abSlots[0]];
      }
    });
  }

  void _pickIr() {
    final path = pickTuningIrFile();
    if (path == null) {
      return;
    }
    setState(() => _conv = path);
    _commit();
  }

  void _clearIr() {
    setState(() => _conv = null);
    _commit();
  }

  Future<String?> _askName({required String title, required String initial}) {
    final controller = TextEditingController(text: initial);
    return showDialog<String>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text(title, style: const TextStyle(fontSize: 14)),
        content: TextField(
          controller: controller,
          autofocus: true,
          style: const TextStyle(fontSize: 12),
          decoration: const InputDecoration(
            isDense: true,
            hintText: '预设名',
          ),
          onSubmitted: (value) => Navigator.of(context).pop(value),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(),
            child: const Text('取消', style: TextStyle(fontSize: 12)),
          ),
          TextButton(
            onPressed: () => Navigator.of(context).pop(controller.text),
            child: const Text('确定', style: TextStyle(fontSize: 12)),
          ),
        ],
      ),
    );
  }

  Future<bool?> _confirm(String title, String message) {
    return showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text(title, style: const TextStyle(fontSize: 14)),
        content: Text(message, style: const TextStyle(fontSize: 12)),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('取消', style: TextStyle(fontSize: 12)),
          ),
          TextButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text('覆盖', style: TextStyle(fontSize: 12)),
          ),
        ],
      ),
    );
  }

  // ── 频响曲线窗（v1 是独立小窗；v2 用对话框，同一套几何）───────────

  void _openCurve() {
    final state = _state;
    if (state == null) {
      return;
    }
    showDialog<void>(
      context: context,
      builder: (context) => _CurveDialog(
        curve: _curveOf(state),
        freqs: state.response.freqs,
        draft: List<TuningFilter>.of(_slots[_slot]),
        slotLabel: _slot == 0 ? '全局' : _channelName(_slot - 1),
        onEdit: (next, {required bool commit}) {
          setState(() {
            _slots[_slot]
              ..clear()
              ..addAll(next);
          });
          if (commit) {
            _commit();
          }
        },
      ),
    );
  }

  /// 当前槽该画哪条线：全局 = 宿主那条全局响应；通道槽 = 宿主按声道算好的那条
  /// （通道还没配段时宿主不会算 ⇒ 退回全局那条）。
  List<double> _curveOf(TuningState state) {
    if (_slot == 0) {
      return state.response.db;
    }
    final index = state.ifElse.indexWhere((b) => b.channel == _slot - 1);
    if (index < 0 || index >= state.condDb.length) {
      return state.response.db;
    }
    final row = state.condDb[index];
    return row.isEmpty ? state.response.db : row;
  }
}

// ── 频响曲线窗 ─────────────────────────────────────────────────────

/// 只画不算：曲线来自宿主（引擎解析式），手柄位置来自草稿。
class _CurveDialog extends StatefulWidget {
  const _CurveDialog({
    required this.curve,
    required this.freqs,
    required this.draft,
    required this.slotLabel,
    required this.onEdit,
  });

  final List<double> curve;
  final List<double> freqs;
  final List<TuningFilter> draft;
  final String slotLabel;
  final void Function(List<TuningFilter> next, {required bool commit}) onEdit;

  @override
  State<_CurveDialog> createState() => _CurveDialogState();
}

class _CurveDialogState extends State<_CurveDialog> {
  late final List<TuningFilter> _draft = List<TuningFilter>.of(widget.draft);
  int? _dragIndex;
  Size _size = const Size(720, 320);

  /// 实测曲线（`lunac_tuning_measure`；null = 还没测）。
  TuningMeasurement? _measured;
  bool _measuring = false;
  String? _measureError;

  // v1 那套几何（pad / 对数横轴 / 线性纵轴），两处（正算 / 反算）必须一致。
  static const double _padL = 30;
  static const double _padR = 8;
  static const double _padT = 8;
  static const double _padB = 16;
  static const double _loF = 20;
  static const double _hiF = 20000;

  /// 横向缩放（v1：滚轮缩放 + 横向滚动）。
  ///
  /// `_zoom = 1` 时窗口就是 20Hz–20kHz 全宽；`_center` 是**可见窗口中点**在整条
  /// 对数轴上的比例（0..1）。只缩横轴 —— 纵轴范围由曲线幅度决定，缩放纵向会让
  /// 「dB 刻度」失去意义。
  double _zoom = 1;
  double _center = 0.5;
  static const double _minZoom = 1;
  static const double _maxZoom = 16;

  double get _loLog => math.log(_loF) / math.ln10;
  double get _hiLog => math.log(_hiF) / math.ln10;
  double get _fullSpan => _hiLog - _loLog;

  /// 可见窗口的对数频率区间（夹在整条轴之内）。
  ({double lo, double hi}) get _window => zoomWindow(
    zoom: _zoom,
    center: _center,
    lo: _loLog,
    hi: _hiLog,
  );

  /// 竖网格线的频点：全宽时用 v1 那五个固定刻度，放大后按十倍频程 + 2/5 倍频
  /// 补齐（最多 12 条，太密就退化成只画十倍频程）。
  List<double> get _gridTicks {
    if (_zoom <= 1.05) {
      return const [20.0, 100.0, 1000.0, 10000.0, 20000.0];
    }
    final w = _window;
    final ticks = <double>[];
    final firstDecade = w.lo.floor();
    final lastDecade = w.hi.ceil();
    for (var k = firstDecade; k <= lastDecade; k++) {
      for (final mult in [1.0, 2.0, 5.0]) {
        final f = mult * math.pow(10, k).toDouble();
        if (f >= w.lo * 1.001 && f <= w.hi * 0.999) {
          ticks.add(f);
        }
      }
    }
    if (ticks.length > 12) {
      return [
        for (var k = firstDecade; k <= lastDecade; k++)
          if (math.pow(10, k).toDouble() >= w.lo &&
              math.pow(10, k).toDouble() <= w.hi)
            math.pow(10, k).toDouble(),
      ];
    }
    return ticks;
  }

  void _resetZoom() {
    setState(() {
      _zoom = 1;
      _center = 0.5;
    });
  }

  /// 滚轮 / 触摸板：默认缩放（以光标为锚点）；`Shift` 或横向滚动 = 平移。
  void _onScroll(PointerScrollEvent event) {
    final plotW = math.max(1.0, _size.width - _padL - _padR);
    final anchorFrac = ((event.localPosition.dx - _padL) / plotW).clamp(0.0, 1.0);
    final window = _window;
    final anchorLog = window.lo + anchorFrac * (window.hi - window.lo);
    final shift = HardwareKeyboard.instance.isShiftPressed;
    if (shift || event.scrollDelta.dx.abs() > event.scrollDelta.dy.abs()) {
      final delta = (shift ? event.scrollDelta.dy : event.scrollDelta.dx) / plotW;
      setState(() => _center = (_center + delta).clamp(0.0, 1.0));
      return;
    }
    final next = (_zoom * math.exp(-event.scrollDelta.dy / 320)).clamp(
      _minZoom,
      _maxZoom,
    );
    if (next == _zoom) {
      return;
    }
    setState(() {
      _zoom = next;
      final width = _fullSpan / _zoom;
      final lo = anchorLog - anchorFrac * width;
      _center = ((lo + width / 2) - _loLog) / _fullSpan;
    });
  }

  double _xOf(double f) {
    final plotW = math.max(1.0, _size.width - _padL - _padR);
    final w = _window;
    return _padL +
        ((math.log(math.max(1, f)) / math.ln10) - w.lo) /
            (w.hi - w.lo) *
            plotW;
  }

  double get _span {
    var maxAbs = 0.0;
    for (final v in widget.curve) {
      if (v.isFinite && v.abs() > maxAbs) {
        maxAbs = v.abs();
      }
    }
    // 实测那条线也要**装得下**：只按合成曲线定纵轴范围的话，实测偏出去的部分会被
    // 切在框外 —— 而「偏出去多少」恰恰是这次测量要看的东西。
    for (final v in [...?_measured?.db, ...?_measured?.chainDb]) {
      if (v.isFinite && v.abs() > maxAbs) {
        maxAbs = v.abs();
      }
    }
    final span = ((maxAbs + 1) / 6).ceil() * 6.0;
    return span.clamp(12.0, 60.0);
  }

  double _yOf(double db) {
    final plotH = math.max(1.0, _size.height - _padT - _padB);
    return _padT + ((_span - db) / (2 * _span)) * plotH;
  }

  ({double f, double db})? _valueAt(Offset local) {
    final plotW = math.max(1.0, _size.width - _padL - _padR);
    final plotH = math.max(1.0, _size.height - _padT - _padB);
    final w = _window;
    final frac = (local.dx - _padL) / plotW;
    if (frac < 0 || frac > 1) {
      return null;
    }
    final f = math.pow(10, w.lo + frac * (w.hi - w.lo)).toDouble();
    final db = _span - ((local.dy - _padT) / plotH) * (2 * _span);
    return (f: f, db: db);
  }

  int? _nearestHandle(Offset local) {
    var best = -1;
    var bestDistance = 16.0;
    for (var i = 0; i < _draft.length; i++) {
      final filter = _draft[i];
      final point = Offset(_xOf(filter.freqHz), _yOf(filter.gainDb ?? 0));
      final distance = (point - local).distance;
      if (distance < bestDistance) {
        bestDistance = distance;
        best = i;
      }
    }
    return best < 0 ? null : best;
  }

  void _start(DragStartDetails details) {
    setState(() => _dragIndex = _nearestHandle(details.localPosition));
  }

  void _update(DragUpdateDetails details) {
    final value = _valueAt(details.localPosition);
    final index = _dragIndex;
    if (value == null) {
      return;
    }
    if (index == null) {
      return;
    }
    setState(() {
      _draft[index] = _draft[index].copyWith(
        freqHz: _clampFreq(value.f),
        gainDb: _clampGain(value.db),
      );
    });
    widget.onEdit(List<TuningFilter>.of(_draft), commit: false);
  }

  void _end(DragEndDetails details) {
    if (_dragIndex != null) {
      widget.onEdit(List<TuningFilter>.of(_draft), commit: true);
    }
    setState(() => _dragIndex = null);
  }

  /// 空白处按下 = 新增一段（peaking，v1 同口径）。
  void _tap(TapUpDetails details) {
    final value = _valueAt(details.localPosition);
    if (value == null || _nearestHandle(details.localPosition) != null) {
      return;
    }
    setState(() {
      _draft.add(
        TuningFilter(
          kind: 'peaking',
          freqHz: _clampFreq(value.f),
          gainDb: _clampGain(value.db),
          q: 0.707,
        ),
      );
    });
    widget.onEdit(List<TuningFilter>.of(_draft), commit: true);
  }

  /// 跑一次实时测量（两趟 WASAPI 回环，约 6 秒）。
  ///
  /// 失败**不弹窗**：错误行就写在曲线窗标题下面（一次没量成不该打断用户）。
  Future<void> _runMeasure() async {
    if (_measuring) {
      return;
    }
    setState(() {
      _measuring = true;
      _measureError = null;
    });
    final reply = await measureTuning();
    if (!mounted) {
      return;
    }
    setState(() {
      _measuring = false;
      if (reply.ok && reply.data != null) {
        _measured = reply.data;
      } else {
        _measureError = reply.error.isEmpty ? '测量失败（没有给出原因）' : reply.error;
      }
    });
  }

  @override
  Widget build(BuildContext context) {
    return Dialog(
      child: SizedBox(
        width: 760,
        height: 420,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(12, 8, 8, 4),
              child: Row(
                children: [
                  Text(
                    '频响曲线 · ${widget.slotLabel}',
                    style: TextStyle(
                      color: LunacColors.text,
                      fontSize: 13,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                  const SizedBox(width: 8),
                  Text(
                    _measured == null
                        ? '拖动圆点改频率 / 增益；空白处点一下加一段 · 滚轮缩放'
                        : '绿虚线 = 仅自身链（该跟实线贴合）· 蓝点线 = 最终输出（含系统）',
                    style: TextStyle(color: LunacColors.textMuted, fontSize: 10.5),
                  ),
                  const Spacer(),
                  if (_zoom > 1.0)
                    TextButton(
                      onPressed: _resetZoom,
                      style: TextButton.styleFrom(
                        foregroundColor: LunacColors.textDim,
                        padding: const EdgeInsets.symmetric(horizontal: 8),
                        minimumSize: Size.zero,
                        visualDensity: VisualDensity.compact,
                      ),
                      child: Text(
                        '复位缩放（${_zoom.toStringAsFixed(1)}×）',
                        style: const TextStyle(fontSize: 10.5),
                      ),
                    ),
                  // 实时测量（v1 的两趟回环）：会出声约 6 秒，所以按钮上写明。
                  TextButton.icon(
                    onPressed: _measuring ? null : _runMeasure,
                    icon: _measuring
                        ? const SizedBox(
                            width: 12,
                            height: 12,
                            child: CircularProgressIndicator(strokeWidth: 2),
                          )
                        : const Icon(Icons.mic_none, size: 13),
                    label: Text(
                      _measuring ? '测量中…' : '实测（约 6 秒，会出声）',
                      style: const TextStyle(fontSize: 10.5),
                    ),
                    style: TextButton.styleFrom(
                      foregroundColor: LunacColors.textDim,
                      padding: const EdgeInsets.symmetric(horizontal: 8),
                      minimumSize: Size.zero,
                      visualDensity: VisualDensity.compact,
                    ),
                  ),
                  IconButton(
                    tooltip: '关闭',
                    icon: const Icon(Icons.close, size: 16),
                    color: LunacColors.textDim,
                    visualDensity: VisualDensity.compact,
                    onPressed: () => Navigator.of(context).pop(),
                  ),
                ],
              ),
            ),
            if (_measureError != null)
              Padding(
                padding: const EdgeInsets.fromLTRB(12, 0, 12, 2),
                child: Text(
                  _measureError!,
                  style: TextStyle(color: LunacColors.danger, fontSize: 10.5),
                ),
              )
            else if (_measured != null)
              Padding(
                padding: const EdgeInsets.fromLTRB(12, 0, 12, 2),
                child: Text(
                  '实测：有效 ${(_measured!.coverage * 100).round()}% · '
                  '${_measured!.elapsedMs}ms · ${_fmt(_measured!.sampleRate / 1000, 1)}kHz',
                  style: TextStyle(
                    color: LunacColors.textMuted,
                    fontSize: 10,
                  ),
                ),
              ),
            Expanded(
              child: Padding(
                padding: const EdgeInsets.fromLTRB(10, 0, 10, 10),
                child: LayoutBuilder(
                  builder: (context, constraints) {
                    _size = Size(constraints.maxWidth, constraints.maxHeight);
                    return Listener(
                      // 滚轮缩放 / Shift+滚轮（或触摸板横向）平移，v1 曲线窗同口径。
                      onPointerSignal: (event) {
                        if (event is PointerScrollEvent) {
                          _onScroll(event);
                        }
                      },
                      child: GestureDetector(
                        onPanStart: _start,
                        onPanUpdate: _update,
                        onPanEnd: _end,
                        onTapUp: _tap,
                        child: CustomPaint(
                          painter: _CurvePainter(
                            curve: widget.curve,
                            freqs: widget.freqs,
                            filters: _draft,
                            span: _span,
                            xOf: _xOf,
                            yOf: _yOf,
                            padL: _padL,
                            padT: _padT,
                            ticks: _gridTicks,
                            plotW: math.max(
                              1.0,
                              constraints.maxWidth - _padL - _padR,
                            ),
                            plotH: math.max(
                              1.0,
                              constraints.maxHeight - _padT - _padB,
                            ),
                            measured: _measured,
                          ),
                          child: const SizedBox.expand(),
                        ),
                      ),
                    );
                  },
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// 网格 + 合成曲线 + 手柄。
class _CurvePainter extends CustomPainter {
  const _CurvePainter({
    required this.curve,
    required this.freqs,
    required this.filters,
    required this.span,
    required this.xOf,
    required this.yOf,
    required this.padL,
    required this.padT,
    required this.plotW,
    required this.plotH,
    required this.ticks,
    this.measured,
  });

  final List<double> curve;
  final List<double> freqs;
  final List<TuningFilter> filters;
  final double span;
  final double Function(double) xOf;
  final double Function(double) yOf;
  final double padL;
  final double padT;
  final double plotW;
  final double plotH;

  /// 竖网格线的频点（随横向缩放变化，见 `_CurveDialogState._gridTicks`）。
  final List<double> ticks;

  /// 实测曲线（null = 还没测；与 v1 一样，两条实测线都在这里画）。
  final TuningMeasurement? measured;

  @override
  void paint(Canvas canvas, Size size) {
    final grid = Paint()
      ..color = LunacColors.border.withValues(alpha: 0.55)
      ..strokeWidth = 1;
    final dim = TextStyle(color: LunacColors.textMuted, fontSize: 9);

    // 横线：0dB 实线（「直通」这条基准），其余虚线。
    for (final v in [span, span / 2, 0.0, -span / 2, -span]) {
      final paint = Paint()
        ..color = grid.color.withValues(alpha: v == 0 ? 0.9 : 0.45)
        ..strokeWidth = 1;
      if (v != 0) {
        _dashedLine(
          canvas,
          Offset(padL, yOf(v)),
          Offset(padL + plotW, yOf(v)),
          paint,
        );
      } else {
        canvas.drawLine(Offset(padL, yOf(v)), Offset(padL + plotW, yOf(v)), paint);
      }
    }
    // 竖线：随缩放变化的刻度（全宽时就是 20 / 100 / 1k / 10k / 20k）。
    for (final f in ticks) {
      final x = xOf(f);
      canvas.drawLine(
        Offset(x, padT),
        Offset(x, padT + plotH),
        Paint()..color = grid.color.withValues(alpha: 0.45),
      );
      final label = f >= 1000
          ? '${(f / 1000).toStringAsFixed(f % 1000 == 0 ? 0 : 1)}k'
          : f.round().toString();
      _text(canvas, label, Offset(x, padT + plotH + 8), dim,
          align: f <= ticks.first * 1.001
              ? TextAlign.left
              : f >= ticks.last * 0.999
                  ? TextAlign.right
                  : TextAlign.center);
    }
    // 纵轴刻度。
    for (final v in [span, span / 2, 0.0, -span / 2, -span]) {
      _text(
        canvas,
        '${v > 0 ? '+' : ''}${_fmt(v, 0)}',
        Offset(padL - 3, yOf(v)),
        dim,
        align: TextAlign.right,
      );
    }

    // 放大后曲线会超出可视窗口 ⇒ 统一裁到绘图区（否则线会画到坐标轴刻度上）。
    canvas.save();
    canvas.clipRect(Rect.fromLTWH(padL, padT, plotW, plotH));

    // 合成曲线（宿主解析式）+ 淡填充。
    if (freqs.length >= 2 && curve.length == freqs.length) {
      final path = Path();
      for (var i = 0; i < freqs.length; i++) {
        final point = Offset(xOf(freqs[i]), yOf(curve[i]));
        if (i == 0) {
          path.moveTo(point.dx, point.dy);
        } else {
          path.lineTo(point.dx, point.dy);
        }
      }
      final fill = Path.from(path)
        ..lineTo(xOf(freqs.last), yOf(0))
        ..lineTo(xOf(freqs.first), yOf(0))
        ..close();
      canvas.drawPath(
        fill,
        Paint()..color = LunacColors.accent.withValues(alpha: 0.12),
      );
      canvas.drawPath(
        path,
        Paint()
          ..color = LunacColors.accent
          ..strokeWidth = 1.5
          ..style = PaintingStyle.stroke,
      );
    }

    // 两条**实测**曲线（都是虚线 / 点线，与合成曲线的实线区分）：
    //   · 仅自身链（绿虚线）= 「调音有没有真的生效」的判据（该跟合成曲线贴合）；
    //   · 最终输出（蓝点线）= 链 × 系统（离合成曲线远**不是**本插件的错）。
    final measured = this.measured;
    if (measured != null && measured.freqs.length >= 2) {
      _measuredLine(
        canvas,
        measured.freqs,
        measured.db,
        const Color(0xFF9490B4),
        const [1.5, 3],
        1.2,
      );
      _measuredLine(
        canvas,
        measured.freqs,
        measured.chainDb,
        const Color(0xFF8FC99A),
        const [5, 3],
        1.5,
      );
    }

    // 手柄：位置来自草稿，拖动时立刻跟手（窗口外的不画，见上面的裁剪）。
    for (final filter in filters) {
      final center = Offset(xOf(filter.freqHz), yOf(filter.gainDb ?? 0));
      canvas.drawCircle(
        center,
        4.5,
        Paint()
          ..color = filter.on
              ? LunacColors.accent
              : LunacColors.textMuted,
      );
      canvas.drawCircle(
        center,
        4.5,
        Paint()
          ..color = LunacColors.surface
          ..strokeWidth = 1
          ..style = PaintingStyle.stroke,
      );
    }
    canvas.restore();
  }

  void _measuredLine(
    Canvas canvas,
    List<double> freqs,
    List<double> values,
    Color color,
    List<double> dash,
    double width,
  ) {
    if (freqs.length < 2 || values.length < 2) {
      return;
    }
    final paint = Paint()
      ..color = color
      ..strokeWidth = width
      ..style = PaintingStyle.stroke;
    // 逐段画（虚线靠「只画一段」实现；与网格那几条共用一个思路）。
    var draw = true;
    var remaining = 0.0;
    var dashIndex = 0;
    Offset? previous;
    for (var i = 0; i < freqs.length && i < values.length; i++) {
      final point = Offset(xOf(freqs[i]), yOf(values[i]));
      if (previous != null) {
        var from = previous;
        var left = (point - previous).distance;
        if (left > 0) {
          final direction = (point - previous) / left;
          while (left > 0) {
            if (remaining <= 0) {
              draw = !draw;
              remaining = dash[dashIndex % dash.length];
              dashIndex += 1;
            }
            final step = math.min(left, remaining);
            if (draw) {
              canvas.drawLine(from, from + direction * step, paint);
            }
            from = from + direction * step;
            left -= step;
            remaining -= step;
          }
        }
      }
      previous = point;
    }
  }

  void _dashedLine(Canvas canvas, Offset from, Offset to, Paint paint) {
    const dash = 3.0;
    const gap = 3.0;
    final total = (to - from).distance;
    if (total <= 0) {
      return;
    }
    final direction = (to - from) / total;
    var travelled = 0.0;
    while (travelled < total) {
      final end = math.min(travelled + dash, total);
      canvas.drawLine(
        from + direction * travelled,
        from + direction * end,
        paint,
      );
      travelled = end + gap;
    }
  }

  void _text(
    Canvas canvas,
    String value,
    Offset at,
    TextStyle style, {
    TextAlign align = TextAlign.left,
  }) {
    final painter = TextPainter(
      text: TextSpan(text: value, style: style),
      textDirection: TextDirection.ltr,
      textAlign: align,
    )..layout();
    final dx = switch (align) {
      TextAlign.right => at.dx - painter.width,
      TextAlign.center => at.dx - painter.width / 2,
      _ => at.dx,
    };
    painter.paint(canvas, Offset(dx, at.dy - painter.height / 2));
  }

  @override
  bool shouldRepaint(covariant _CurvePainter old) =>
      old.curve != curve ||
      old.filters.length != filters.length ||
      old.span != span ||
      old.plotW != plotW ||
      old.plotH != plotH ||
      old.ticks != ticks ||
      !identical(old.measured, measured);
}

// ── 数字输入框 ─────────────────────────────────────────────────────

/// 一个「按失焦 / 回车提交」的数字框（v1 同口径：不是每敲一个字发一次）。
class _NumField extends StatefulWidget {
  const _NumField({
    required this.value,
    required this.decimals,
    required this.onSubmit,
    this.width = 60,
    this.big = false,
  });

  final double value;
  final int decimals;
  final ValueChanged<double> onSubmit;
  final double width;
  final bool big;

  @override
  State<_NumField> createState() => _NumFieldState();
}

class _NumFieldState extends State<_NumField> {
  late final TextEditingController _controller = TextEditingController(
    text: _fmt(widget.value, widget.decimals),
  );
  final FocusNode _focus = FocusNode();

  @override
  void initState() {
    super.initState();
    _focus.addListener(() {
      if (!_focus.hasFocus) {
        _submit();
      }
    });
  }

  @override
  void didUpdateWidget(covariant _NumField old) {
    super.didUpdateWidget(old);
    if (!_focus.hasFocus && widget.value != old.value) {
      _controller.text = _fmt(widget.value, widget.decimals);
    }
  }

  @override
  void dispose() {
    _controller.dispose();
    _focus.dispose();
    super.dispose();
  }

  void _submit() {
    final parsed = double.tryParse(_controller.text.trim().replaceAll(',', '.'));
    if (parsed == null) {
      // 非法输入 ⇒ 恢复成草稿值（不改草稿、不发 IPC）。
      _controller.text = _fmt(widget.value, widget.decimals);
      return;
    }
    if ((parsed - widget.value).abs() < 1e-9) {
      return;
    }
    widget.onSubmit(parsed);
  }

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      width: widget.width,
      height: widget.big ? 22 : 20,
      child: TextField(
        controller: _controller,
        focusNode: _focus,
        style: TextStyle(
          color: LunacColors.text,
          fontSize: widget.big ? 12.5 : 10.5,
          fontWeight: widget.big ? FontWeight.w600 : FontWeight.w400,
        ),
        textAlign: TextAlign.center,
        decoration: const InputDecoration(
          isDense: true,
          contentPadding: EdgeInsets.symmetric(horizontal: 2, vertical: 2),
          border: OutlineInputBorder(),
        ),
        onSubmitted: (_) => _submit(),
      ),
    );
  }
}
