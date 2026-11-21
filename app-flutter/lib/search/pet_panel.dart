// 桌宠控制台面板（v1 `app/src/plugins/builtin/pet.ts` 的「控制台」那一半）。
//
// 与设置 / 对话 / 备忘录面板同高（640 档）、同外框；Esc / 左上角返回关闭。
//
// v1 的口径（先读源码确认过的）：
//   · **穿透开关只能放在控制台** —— 穿透开着时桌宠窗收不到任何鼠标事件，
//     开关若只放在桌宠窗里，用户一按就再也关不掉；
//   · 形象由用户导入（图片 png/jpg/webp/gif，或 Live2D `*.model3.json`），二选一，
//     `model` 优先；
//   · 配置字段名与 localStorage 键 `lunac.pet.v1` 一致（v2 落 ModuleData/pet/）。
//
// v2 已接：窗口 + 形象、「模型调试」（表情 / 9 个常用参数）、「主动反应」、「预设动作」、
// 「热键」、「语音朗读」（配置 + 资产体检；合成通路未移植，页内如实标注）、「桌宠人格」。
import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../platform/agent.dart' show startAgent, stopAgent;
import '../platform/pet.dart';
import '../theme.dart';

/// v1 控制台的六个页签（顺序与文案逐字取 `pet.ts` 的 `settingsHtml` + i18n）。
const List<({String id, String label})> _petTabs = [
  (id: 'debug', label: '模型调试'),
  (id: 'hotkey', label: '热键'),
  (id: 'motion', label: '预设动作'),
  (id: 'proactive', label: '主动反应'),
  (id: 'voice', label: '语音朗读'),
  (id: 'persona', label: '桌宠人格'),
];

/// 桌宠控制台：搜索「桌宠 / pet / live2d」进入。
class PetPanel extends StatefulWidget {
  const PetPanel({super.key, required this.onClose, this.windowControls});

  /// 关闭面板（Esc / 返回按钮）。
  final VoidCallback onClose;

  /// 独立窗的「置顶 / 最小化 / 关闭」（由 `plugin_window_main` 注入）。
  /// 非 null ⇒ 不再画「返回」，三个窗口键常驻右上角。
  final Widget? windowControls;

  @override
  State<PetPanel> createState() => _PetPanelState();
}

class _PetPanelState extends State<PetPanel> {
  late final FocusNode _panelFocus = FocusNode(
    debugLabel: 'pet-panel',
    onKeyEvent: _onKey,
  );

  PetConfig _config = readPetConfig();
  PetStatus _status = readPetStatus();
  PetModelInfo? _modelInfo;

  /// 桌宠右键菜单「打开控制台」/ 滚轮缩放 / 渲染错误都经事件队列上来。
  Timer? _poll;

  String? _message;
  bool _messageError = false;

  // ── 六个页签（v1 同顺序；本版只有「桌宠人格」有后端）──
  String _tab = 'debug';
  PetPersona? _persona;
  PetMemoryList? _memoryList;
  PetMemoryList? _growthList;
  PetMemoryList? _suggestList;
  late final TextEditingController _personaText = TextEditingController();
  String? _tabMessage;
  bool _tabMessageError = false;

  /// 两段式「清空」：`persona` / `memory` / `growth` / `suggestions`。
  String? _armedClear;
  Timer? _armTimer;

  // ── 主动反应页（v1 `data-pet-page="proactive"`）────────────────────
  late final TextEditingController _proactiveInterval = TextEditingController();
  late final TextEditingController _proactiveCap = TextEditingController();
  late final TextEditingController _proactivePoll = TextEditingController();
  late final TextEditingController _proactiveGap = TextEditingController();
  late final TextEditingController _proactiveDiff = TextEditingController();
  Timer? _proactiveDebounce;

  // ── 预设动作页（v1 `data-pet-page="motion"`）──────────────────────
  List<PetMotionFile> _motionFiles = const [];
  String? _motionMessage;
  bool _motionError = false;
  bool _motionBusy = false;

  /// 热键页（`lunac-live2d/lunac.hotkeys.json`）。
  List<PetHotkey> _hotkeys = const [];
  String? _hotkeyMessage;
  bool _hotkeyError = false;
  bool _hotkeyBusy = false;
  int _hotkeySeq = 0;

  /// 语音朗读页（`config/pet-voice.json`）。
  PetVoiceStatus _voice = const PetVoiceStatus();
  late final TextEditingController _voiceText = TextEditingController();
  late final TextEditingController _voiceServer = TextEditingController(
    text: 'http://127.0.0.1:9880',
  );
  late final TextEditingController _voiceSteps = TextEditingController(
    text: '4',
  );
  late final TextEditingController _voiceSpeed = TextEditingController(
    text: '1.0',
  );
  late final TextEditingController _voiceVolume = TextEditingController(
    text: '100',
  );
  String _voiceEngine = 'gpt_sovits';
  String? _voiceMessage;
  bool _voiceError = false;
  bool _voiceBusy = false;
  Map<String, dynamic>? _coreIds;

  // ── 模型调试页（v1 `data-pet-page="debug"`）──────────────────────
  //
  // v1 的调试区列出模型的全部自由参数；v2 先做 v1 `DEBUG_PARAMS` 那 9 个常用项
  // （id / 区间 / 中文名逐字一致），经 `lunac_pet_debug` 走 WebView 引擎的
  // `setParameter` / `setExpression`（与 v1 同一份 live2d-engine）。
  static const List<({String id, double min, double max, String label})>
  _debugParams = [
    (id: 'ParamAngleX', min: -100, max: 100, label: '头部左右'),
    (id: 'ParamAngleY', min: -100, max: 100, label: '头部上下'),
    (id: 'ParamAngleZ', min: -100, max: 100, label: '头部倾斜'),
    (id: 'ParamEyeLOpen', min: 0, max: 100, label: '左眼开合'),
    (id: 'ParamEyeROpen', min: 0, max: 100, label: '右眼开合'),
    (id: 'ParamMouthOpenY', min: 0, max: 100, label: '嘴张开'),
    (id: 'ParamBodyAngleX', min: -100, max: 100, label: '身体左右'),
    (id: 'ParamBodyAngleZ', min: -100, max: 100, label: '身体倾斜'),
    (id: 'ParamBreath', min: 0, max: 100, label: '呼吸'),
  ];

  /// 当前模型持久化的参数值（拖动时即时改；与盘上一致）。
  Map<String, double> _debugValues = {};

  /// 当前模型生效的表情名（空串 = 已还原）。
  String _activeExpression = '';

  void _syncDebugState() {
    final model = _config.model;
    _debugValues = {...?_config.params[model]};
    _activeExpression = _config.expressions[model] ?? '';
  }

  @override
  void initState() {
    super.initState();
    _refreshModelInfo();
    _syncDebugState();
    _loadPersona();
    _loadLists();
    _refreshHotkeys();
    _refreshVoice();
    _poll = Timer.periodic(const Duration(milliseconds: 700), (_) {
      final events = pollPetEvents();
      if (!mounted || events.isEmpty) {
        return;
      }
      setState(() {
        for (final event in events) {
          if (event.config != null) {
            _config = event.config!;
          }
          if (event.message != null && event.message!.isNotEmpty) {
            _status = PetStatus(open: _status.open, error: event.message);
          }
          if (event.coreIds != null) {
            _coreIds = event.coreIds;
          }
          if (event.type == 'closed') {
            _status = const PetStatus();
          }
        }
      });
    });
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) {
        _panelFocus.requestFocus();
      }
    });
  }

  @override
  void dispose() {
    _poll?.cancel();
    _armTimer?.cancel();
    _proactiveDebounce?.cancel();
    _personaText.dispose();
    _voiceText.dispose();
    _voiceServer.dispose();
    _voiceSteps.dispose();
    _voiceSpeed.dispose();
    _voiceVolume.dispose();
    _proactiveInterval.dispose();
    _proactiveCap.dispose();
    _proactivePoll.dispose();
    _proactiveGap.dispose();
    _proactiveDiff.dispose();
    _panelFocus.dispose();
    super.dispose();
  }

  KeyEventResult _onKey(FocusNode node, KeyEvent event) {
    if (event is KeyDownEvent &&
        event.logicalKey == LogicalKeyboardKey.escape) {
      widget.onClose();
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  void _refreshModelInfo() {
    if (!_config.isLive2D) {
      _modelInfo = null;
      return;
    }
    _modelInfo = validatePetModel(_config.model);
  }

  void _toast(String text, {bool error = false}) {
    setState(() {
      _message = text;
      _messageError = error;
    });
  }

  void _syncStatus(PetStatus status) {
    setState(() {
      _status = status;
      if (status.error != null && status.error!.isNotEmpty) {
        _message = status.error;
        _messageError = true;
      }
    });
  }

  // ── 桌宠人格页（v1 `pet-persona` 段 + 三份列表）──────────────────

  void _tabToast(String text, {bool error = false}) {
    setState(() {
      _tabMessage = text;
      _tabMessageError = error;
    });
  }

  void _loadPersona() {
    final persona = readPetPersona();
    if (!mounted) {
      return;
    }
    setState(() {
      _persona = persona;
      _personaText.text = persona.text;
      if (!persona.ok && persona.error.isNotEmpty) {
        _tabMessage = persona.error;
        _tabMessageError = true;
      }
    });
  }

  void _loadLists() {
    final memory = readPetMemory(PetMemoryKind.memory);
    final growth = readPetMemory(PetMemoryKind.growth);
    final suggestions = readPetMemory(PetMemoryKind.suggestions);
    if (!mounted) {
      return;
    }
    setState(() {
      _memoryList = memory;
      _growthList = growth;
      _suggestList = suggestions;
    });
  }

  void _savePersona() {
    final error = savePetPersona(_personaText.text);
    _tabToast(error ?? '人格已保存', error: error != null);
    if (error == null) {
      _loadPersona();
    }
  }

  void _clearPersona() {
    if (!_arm('persona')) {
      return;
    }
    _personaText.text = '';
    final error = savePetPersona('');
    _tabToast(error ?? '已清空桌宠人格（之后可重新填写）', error: error != null);
    if (error == null) {
      _loadPersona();
    }
  }

  void _restartAi() {
    stopAgent();
    final reason = startAgent();
    _tabToast(reason ?? 'AI 已重启（下一次提问使用新人格）', error: reason != null);
  }

  void _deleteEntry(PetMemoryKind kind, String entry) {
    final error = deletePetMemoryEntry(kind, entry);
    if (error == null) {
      _loadLists();
    }
    _tabToast(error ?? '已删除一条', error: error != null);
  }

  void _clearList(PetMemoryKind kind, String armedNote) {
    if (!_arm(kind.name, note: armedNote)) {
      return;
    }
    final error = clearPetMemory(kind);
    if (error == null) {
      _loadLists();
    }
    _tabToast(error ?? '已清空', error: error != null);
  }

  void _acceptSuggestion(String entry) {
    final error = mergePetPersonaSuggestion(entry);
    if (error == null) {
      _loadPersona();
      _loadLists();
    }
    _tabToast(error ?? '已并入核心人格', error: error != null);
  }

  /// 两段式确认（与 v1 `data-armed-text` 同口径）：第一次点只上膛 3 秒。
  bool _arm(String key, {String note = '再点一次清空（不可恢复）'}) {
    if (_armedClear != key) {
      _armTimer?.cancel();
      setState(() => _armedClear = key);
      _tabToast(note);
      _armTimer = Timer(const Duration(seconds: 3), () {
        if (mounted) {
          setState(() => _armedClear = null);
        }
      });
      return false;
    }
    _armTimer?.cancel();
    setState(() => _armedClear = null);
    return true;
  }

  // ── 动作 ────────────────────────────────────────────────────────

  void _show() {
    _syncStatus(showPetWindow());
    _toast('已显示桌宠');
  }

  void _hide() {
    _syncStatus(hidePetWindow());
    _toast('已隐藏桌宠（控制台里随时能再显示）');
  }

  void _quit() {
    _syncStatus(quitPetWindow());
    _toast('已退出桌宠');
  }

  void _resetPosition() {
    _syncStatus(resetPetPosition());
    _toast('桌宠窗已回到默认位置');
  }

  void _toggleTopmost(bool on) {
    setState(() => _config = _config.copyWith(topmost: on));
    _syncStatus(setPetTopmost(on));
  }

  void _toggleClickThrough(bool on) {
    setState(() => _config = _config.copyWith(clickThrough: on));
    _syncStatus(setPetClickThrough(on));
    _toast(on ? '已开启鼠标穿透：桌宠不再吃鼠标事件（只能在这里关掉）' : '已关闭鼠标穿透');
  }

  void _commitScale(double value) {
    final saved = savePetConfig({'scale': value});
    setState(() => _config = saved);
  }

  /// v1 `.pet-row` 的旋转角度（只对 Live2D 模型生效）。
  void _commitRotation(double value) {
    final saved = savePetConfig({'rotation': value});
    setState(() => _config = saved);
  }

  /// v1 `pet-clear`：清掉形象配置，并把桌宠窗收起。
  void _clearShape() {
    setState(() {
      _config = savePetConfig({'model': '', 'image': ''});
      _modelInfo = null;
      _syncDebugState();
    });
    _syncStatus(hidePetWindow());
    _toast('已取消形象配置');
  }

  Future<void> _import() async {
    final path = pickPetShapeFile();
    if (path == null) {
      return;
    }
    if (isLive2DModelPath(path)) {
      final info = validatePetModel(path);
      if (info == null || !info.ok) {
        _toast('模型不可用：${info?.reason ?? '桥接不可用'}', error: true);
        return;
      }
      setState(() {
        _config = savePetConfig({'model': path, 'image': ''});
        _modelInfo = info;
        _syncDebugState();
      });
      _toast('已导入 Live2D 模型（${info.reason}）');
      return;
    }
    if (!isPetImagePath(path)) {
      _toast('只支持 png / jpg / jpeg / webp / gif 或 *.model3.json', error: true);
      return;
    }
    setState(() {
      _config = savePetConfig({'model': '', 'image': path});
      _modelInfo = null;
    });
    _toast('已导入图片形象');
  }

  // ── 界面 ────────────────────────────────────────────────────────

  @override
  Widget build(BuildContext context) {
    return Focus(
      focusNode: _panelFocus,
      autofocus: true,
      onKeyEvent: _onKey,
      child: Container(
        decoration: BoxDecoration(
          color: LunacColors.surface,
          border: Border.all(color: LunacColors.border),
          borderRadius: const BorderRadius.vertical(
            bottom: Radius.circular(12),
          ),
        ),
        foregroundDecoration: BoxDecoration(
          border: Border(top: BorderSide(color: LunacColors.divider)),
        ),
        clipBehavior: Clip.antiAlias,
        child: Column(
          children: [
            _buildHeader(),
            Expanded(
              child: SingleChildScrollView(
                padding: const EdgeInsets.fromLTRB(14, 4, 14, 16),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    _buildMainRow(),
                    _buildSliderRow(
                      label: '形象大小',
                      value: _config.scale.clamp(0.4, 8),
                      min: 0.4,
                      max: 8,
                      divisions: 76,
                      valueText: '${(_config.scale * 100).round()}%',
                      onChanged: (value) => setState(
                        () => _config = _config.copyWith(scale: value),
                      ),
                      onChangeEnd: _commitScale,
                    ),
                    _buildSliderRow(
                      label: '旋转角度',
                      value: _config.rotation.clamp(-180.0, 180.0),
                      min: -180,
                      max: 180,
                      divisions: 360,
                      valueText: '${_config.rotation.round()}°',
                      // v1：旋转只对 Live2D 模型有意义（图片走 CSS transform）。
                      enabled: _config.isLive2D,
                      onChanged: (value) => setState(
                        () => _config = _config.copyWith(rotation: value),
                      ),
                      onChangeEnd: _commitRotation,
                    ),
                    _buildFileBlock(),
                    _buildNotes(),
                    _buildTabBar(),
                    _buildTabPage(),
                    _buildExtras(),
                    if (_message != null) _buildMessage(),
                  ],
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildHeader() {
    return Padding(
      padding: const EdgeInsets.fromLTRB(10, 8, 12, 6),
      child: Row(
        children: [
          // 内嵌模式才画「返回」；独立窗的三个窗口键放**右上角**（用户 2026-11-20 口径）。
          if (widget.windowControls == null) ...[
            IconButton(
              tooltip: '返回（Esc）',
              icon: const Icon(Icons.arrow_back, size: 18),
              color: LunacColors.textDim,
              visualDensity: VisualDensity.compact,
              onPressed: widget.onClose,
            ),
            const SizedBox(width: 2),
          ],
          Text(
            '桌宠',
            style: TextStyle(
              color: LunacColors.text,
              fontSize: 14,
              fontWeight: FontWeight.w600,
            ),
          ),
          const SizedBox(width: 8),
          Text(
            _status.open ? '桌宠窗已打开' : '桌宠窗未打开',
            style: TextStyle(color: LunacColors.textMuted, fontSize: 12),
          ),
          if (widget.windowControls != null)
            Expanded(
              child: Align(
                alignment: Alignment.centerRight,
                child: widget.windowControls!,
              ),
            ),
        ],
      ),
    );
  }

  Widget _button(
    String label,
    VoidCallback onPressed, {
    bool primary = false,
    bool danger = false,
    bool active = false,
  }) {
    return FilledButton(
      onPressed: onPressed,
      style: FilledButton.styleFrom(
        backgroundColor: primary
            ? LunacColors.accent
            : (active
                  ? LunacColors.accent.withValues(alpha: 0.18)
                  : LunacColors.surfaceHover),
        foregroundColor: primary
            ? LunacColors.background
            : (danger ? LunacColors.danger : LunacColors.text),
        side: active && !primary
            ? BorderSide(color: LunacColors.accent.withValues(alpha: 0.6))
            : BorderSide.none,
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
        textStyle: const TextStyle(fontSize: 12.5),
      ),
      child: Text(label),
    );
  }

  /// v1 控制台首行：显示 / 隐藏、更换模型、取消配置、鼠标穿透（`.pet-row-main`）。
  Widget _buildMainRow() {
    final hasShape = _config.shapePath.isNotEmpty;
    return Wrap(
      spacing: 8,
      runSpacing: 8,
      children: [
        _button(
          _status.open ? '隐藏模型' : '显示桌宠',
          _status.open ? _hide : _show,
          primary: true,
        ),
        _button('更换模型…', _import),
        if (hasShape) _button('取消配置', _clearShape),
        _button(
          _config.clickThrough ? '鼠标穿透：开' : '鼠标穿透：关',
          () => _toggleClickThrough(!_config.clickThrough),
          active: _config.clickThrough,
        ),
      ],
    );
  }

  /// v1 `.pet-row`：标签 + 滑块 + 数值（形象大小 / 旋转角度）。
  Widget _buildSliderRow({
    required String label,
    required double value,
    required double min,
    required double max,
    required int divisions,
    required String valueText,
    required ValueChanged<double> onChanged,
    ValueChanged<double>? onChangeEnd,
    bool enabled = true,
  }) {
    return Padding(
      padding: const EdgeInsets.only(top: 10),
      child: Row(
        children: [
          SizedBox(
            width: 64,
            child: Text(
              label,
              style: TextStyle(color: LunacColors.textDim, fontSize: 12),
            ),
          ),
          Expanded(
            child: Slider(
              min: min,
              max: max,
              divisions: divisions,
              value: value,
              label: valueText,
              onChanged: enabled ? onChanged : null,
              onChangeEnd: enabled ? onChangeEnd : null,
            ),
          ),
          SizedBox(
            width: 52,
            child: Text(
              valueText,
              textAlign: TextAlign.right,
              style: TextStyle(color: LunacColors.textDim, fontSize: 12),
            ),
          ),
        ],
      ),
    );
  }

  /// 当前形象（v1 `.pet-file` 块：路径 + 模型校验结论）。
  Widget _buildFileBlock() {
    final path = _config.shapePath;
    return Container(
      margin: const EdgeInsets.only(top: 12),
      padding: const EdgeInsets.all(9),
      decoration: BoxDecoration(
        color: LunacColors.background,
        border: Border.all(color: LunacColors.border),
        borderRadius: BorderRadius.circular(8),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            path.isEmpty ? '还没有形象' : (_config.isLive2D ? 'Live2D 模型' : '图片形象'),
            style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
          ),
          const SizedBox(height: 4),
          SelectableText(
            path.isEmpty
                ? '点「更换模型…」选一张图片（png / jpg / webp / gif）或一个 *.model3.json'
                : path,
            style: TextStyle(
              color: path.isEmpty ? LunacColors.textMuted : LunacColors.textDim,
              fontSize: 11.5,
              height: 1.45,
            ),
          ),
          if (_modelInfo != null) ...[
            const SizedBox(height: 5),
            Text(
              _modelInfo!.ok
                  ? 'Cubism ${_modelInfo!.version} · 纹理 ${_modelInfo!.textures} · 动作组 ${_modelInfo!.reason}'
                  : '模型不可用：${_modelInfo!.reason}',
              style: TextStyle(
                color: _modelInfo!.ok
                    ? LunacColors.textMuted
                    : LunacColors.danger,
                fontSize: 11,
              ),
            ),
          ],
        ],
      ),
    );
  }

  /// v1 控制台的提示行（`.pet-note`）。
  Widget _buildNotes() {
    return Padding(
      padding: EdgeInsets.only(top: 10),
      child: Text(
        '提示：直接拖动是移动窗口；按住 Alt 拖动可移动形象本身，滚轮按指针位置缩放。',
        style: TextStyle(
          color: LunacColors.textMuted,
          fontSize: 11,
          height: 1.45,
        ),
      ),
    );
  }

  /// v1 六页签的页签条（`.pet-tabs`）：当前页高亮，其余可点切换。
  Widget _buildTabBar() {
    return Padding(
      padding: const EdgeInsets.only(top: 12),
      child: Wrap(
        spacing: 6,
        runSpacing: 6,
        children: [
          for (final tab in _petTabs)
            InkWell(
              onTap: () => setState(() {
                _tab = tab.id;
                _tabMessage = null;
                _armedClear = null;
                if (tab.id == 'debug') {
                  _config = readPetConfig();
                  _refreshModelInfo();
                  _syncDebugState();
                }
                if (tab.id == 'proactive') {
                  _config = readPetConfig();
                  _syncProactiveFields();
                }
                if (tab.id == 'motion') {
                  _config = readPetConfig();
                  _refreshMotionFiles();
                }
                if (tab.id == 'hotkey') {
                  _config = readPetConfig();
                  _refreshHotkeys();
                }
                if (tab.id == 'voice') {
                  _refreshVoice();
                }
              }),
              borderRadius: BorderRadius.circular(6),
              child: Container(
                padding: const EdgeInsets.symmetric(
                  horizontal: 10,
                  vertical: 6,
                ),
                decoration: BoxDecoration(
                  color: _tab == tab.id
                      ? LunacColors.accent.withValues(alpha: 0.16)
                      : Colors.transparent,
                  border: Border.all(
                    color: _tab == tab.id
                        ? LunacColors.accent.withValues(alpha: 0.5)
                        : LunacColors.border,
                  ),
                  borderRadius: BorderRadius.circular(6),
                ),
                child: Text(
                  tab.label,
                  style: TextStyle(
                    color: _tab == tab.id
                        ? LunacColors.text
                        : LunacColors.textDim,
                    fontSize: 11.5,
                  ),
                ),
              ),
            ),
        ],
      ),
    );
  }

  /// 当前页签的内容。六个页签都有对应实现：
  /// 「模型调试」「主动反应」「预设动作」「热键」「语音朗读」（配置 + 资产体检）、「桌宠人格」。
  Widget _buildTabPage() {
    if (_tab == 'persona') {
      return _buildPersonaPage();
    }
    if (_tab == 'debug') {
      return _buildDebugPage();
    }
    if (_tab == 'proactive') {
      return _buildProactivePage();
    }
    if (_tab == 'motion') {
      return _buildMotionPage();
    }
    if (_tab == 'hotkey') {
      return _buildHotkeyPage();
    }
    if (_tab == 'voice') {
      return _buildVoicePage();
    }
    return const SizedBox.shrink();
  }

  /// 「主动反应」页（v1 `data-pet-page="proactive"`）。
  ///
  /// v1 2026-10-22 口径：**只有控件与数字**（两段解释文案已删）。开关默认关 —— 这条链路
  /// 持续烧 token，必须用户显式同意；「0 = 不限次数」由每日上限那个数字框表达。
  Widget _buildProactivePage() {
    final pr = _config.proactive;
    final used = pr.usedToday();
    final capText = pr.dailyCap > 0 ? '${pr.dailyCap}' : '不限';
    final usage =
        '今日已用 $used / $capText'
        '${pr.capReached() ? ' · 已达今日上限，已自动停止' : ''}';
    return Container(
      margin: const EdgeInsets.only(top: 10),
      padding: const EdgeInsets.fromLTRB(10, 8, 10, 10),
      decoration: BoxDecoration(
        color: LunacColors.background,
        border: Border.all(color: LunacColors.border),
        borderRadius: BorderRadius.circular(8),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            '主动反应',
            style: TextStyle(
              color: LunacColors.text,
              fontSize: 12,
              fontWeight: FontWeight.w600,
            ),
          ),
          const SizedBox(height: 6),
          _proactiveToggleRow(pr),
          _proactiveNumberRow(
            label: '最小间隔',
            controller: _proactiveInterval,
            unit: '分钟',
            apply: (value) => pr.copyWith(intervalMin: value),
          ),
          _proactiveNumberRow(
            label: '每日上限',
            controller: _proactiveCap,
            unit: '次/天',
            apply: (value) => pr.copyWith(dailyCap: value),
          ),
          _proactiveNote('0 = 不限次数'),
          _proactiveNumberRow(
            label: '屏幕抓取频次',
            controller: _proactivePoll,
            unit: '秒',
            apply: (value) => pr.copyWith(screenPollSec: value),
          ),
          _proactiveNumberRow(
            label: '屏幕反应最小间隔',
            controller: _proactiveGap,
            unit: '秒',
            apply: (value) => pr.copyWith(screenMinGapSec: value),
          ),
          _proactiveNumberRow(
            label: '屏幕反应灵敏度',
            controller: _proactiveDiff,
            apply: (value) => pr.copyWith(screenDiff: value),
          ),
          _proactiveNote('灵敏度数值越小越敏感（1 最敏感，64 最迟钝）。'),
          const SizedBox(height: 4),
          Text(
            usage,
            style: TextStyle(color: LunacColors.textMuted, fontSize: 10.5),
          ),
          if (_tabMessage != null)
            Padding(
              padding: const EdgeInsets.only(top: 6),
              child: Text(
                _tabMessage!,
                style: TextStyle(
                  color: _tabMessageError
                      ? LunacColors.danger
                      : LunacColors.textMuted,
                  fontSize: 10.5,
                ),
              ),
            ),
        ],
      ),
    );
  }

  /// 主动反应开关（按钮两态文案就是「开 / 关」，与 v1 一致）。
  Widget _proactiveToggleRow(ProactiveConfig pr) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 2),
      child: Row(
        children: [
          SizedBox(
            width: 104,
            child: Text(
              '主动反应',
              style: TextStyle(color: LunacColors.textDim, fontSize: 11),
            ),
          ),
          TextButton(
            onPressed: () => _saveProactive(pr.copyWith(enabled: !pr.enabled)),
            style: TextButton.styleFrom(
              foregroundColor: pr.enabled
                  ? LunacColors.text
                  : LunacColors.textDim,
              backgroundColor: pr.enabled
                  ? LunacColors.surfaceHover
                  : LunacColors.surface,
              side: BorderSide(
                color: pr.enabled ? LunacColors.divider : LunacColors.border,
              ),
              padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 4),
              minimumSize: Size.zero,
              visualDensity: VisualDensity.compact,
              shape: RoundedRectangleBorder(
                borderRadius: BorderRadius.circular(999),
              ),
            ),
            child: Text(
              pr.enabled ? '开' : '关',
              style: const TextStyle(fontSize: 11),
            ),
          ),
        ],
      ),
    );
  }

  /// 一行「中文名 + 数字框 + 单位」（v1 `pet-param-num`）。
  Widget _proactiveNumberRow({
    required String label,
    required TextEditingController controller,
    required ProactiveConfig Function(int value) apply,
    String? unit,
  }) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 2),
      child: Row(
        children: [
          SizedBox(
            width: 104,
            child: Text(
              label,
              style: TextStyle(color: LunacColors.textDim, fontSize: 11),
            ),
          ),
          SizedBox(
            width: 58,
            height: 24,
            child: Focus(
              onFocusChange: (focused) {
                if (!focused) {
                  _commitProactiveField(controller, apply);
                }
              },
              child: TextField(
                controller: controller,
                keyboardType: TextInputType.number,
                inputFormatters: [FilteringTextInputFormatter.digitsOnly],
                textAlign: TextAlign.center,
                style: TextStyle(color: LunacColors.text, fontSize: 11),
                cursorColor: LunacColors.accent,
                decoration: InputDecoration(
                  isDense: true,
                  filled: true,
                  fillColor: LunacColors.surfaceHover,
                  contentPadding: const EdgeInsets.symmetric(
                    horizontal: 6,
                    vertical: 6,
                  ),
                  border: OutlineInputBorder(
                    borderRadius: BorderRadius.circular(4),
                    borderSide: BorderSide(color: LunacColors.border),
                  ),
                  enabledBorder: OutlineInputBorder(
                    borderRadius: BorderRadius.circular(4),
                    borderSide: BorderSide(color: LunacColors.border),
                  ),
                  focusedBorder: OutlineInputBorder(
                    borderRadius: BorderRadius.circular(4),
                    borderSide: BorderSide(color: LunacColors.accent),
                  ),
                ),
                onChanged: (_) {
                  // 与 v1 的「逐次保存」等价，但不打断输入：停止键入 500ms 后写盘。
                  _proactiveDebounce?.cancel();
                  _proactiveDebounce = Timer(
                    const Duration(milliseconds: 500),
                    () {
                      _commitProactiveField(controller, apply);
                    },
                  );
                },
                onSubmitted: (_) => _commitProactiveField(controller, apply),
              ),
            ),
          ),
          if (unit != null) ...[
            const SizedBox(width: 6),
            Text(
              unit,
              style: TextStyle(color: LunacColors.textMuted, fontSize: 10.5),
            ),
          ],
        ],
      ),
    );
  }

  Widget _proactiveNote(String text) {
    return Padding(
      padding: const EdgeInsets.only(left: 104, top: 1),
      child: Text(
        text,
        style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
      ),
    );
  }

  /// 提交一个数字框：空 / 非数字 ⇒ 还原为盘上值并提示；合法值写盘。
  void _commitProactiveField(
    TextEditingController controller,
    ProactiveConfig Function(int value) apply,
  ) {
    _proactiveDebounce?.cancel();
    final parsed = int.tryParse(controller.text.trim());
    final current = _proactiveValueOf(_config.proactive, controller);
    if (parsed == null) {
      controller.text = '$current';
      _tabToast('只能是数字', error: true);
      return;
    }
    if (parsed == current) {
      return;
    }
    _saveProactive(apply(parsed));
  }

  /// 数字框当前对应的配置值（用于「原样还原」）。
  int _proactiveValueOf(ProactiveConfig pr, TextEditingController controller) {
    if (identical(controller, _proactiveInterval)) {
      return pr.intervalMin;
    }
    if (identical(controller, _proactiveCap)) {
      return pr.dailyCap;
    }
    if (identical(controller, _proactivePoll)) {
      return pr.screenPollSec;
    }
    if (identical(controller, _proactiveGap)) {
      return pr.screenMinGapSec;
    }
    return pr.screenDiff;
  }

  /// 保存主动反应配置；成功时静默更新（与 v1 一样不弹「已保存」）。
  void _saveProactive(ProactiveConfig next) {
    final result = savePetProactive(next.toPatch());
    if (!mounted) {
      return;
    }
    setState(() {
      _config = result.config;
      _tabMessage = result.error;
      _tabMessageError = result.error != null;
    });
    // 宿主会把越界值夹进区间（例如间隔 0 → 1）：把归一化后的数字回填到框里。
    _syncProactiveFields();
  }

  /// 把配置里的数字写回输入框（切到本页 / 保存被宿主归一化后调用）。
  void _syncProactiveFields() {
    final pr = _config.proactive;
    void set(TextEditingController controller, int value) {
      final text = '$value';
      if (controller.text != text) {
        controller.text = text;
      }
    }

    set(_proactiveInterval, pr.intervalMin);
    set(_proactiveCap, pr.dailyCap);
    set(_proactivePoll, pr.screenPollSec);
    set(_proactiveGap, pr.screenMinGapSec);
    set(_proactiveDiff, pr.screenDiff);
  }

  // ── 预设动作页（v1 `data-pet-page="motion"`）──────────────────────

  /// 重扫模型目录里的 `*.motion3.json`（行号 = 引擎 `Lunac` 组序号）。
  void _refreshMotionFiles() {
    final model = _config.model.trim();
    if (model.isEmpty) {
      setState(() => _motionFiles = const []);
      return;
    }
    final result = readPetMotionFiles(model);
    setState(() {
      _motionFiles = result.files;
      _motionMessage = result.error;
      _motionError = result.error != null;
    });
  }

  void _motionToast(String text, {bool error = false}) {
    if (!mounted) {
      return;
    }
    setState(() {
      _motionMessage = text;
      _motionError = error;
    });
  }

  void _playMotion(int index) {
    final result = playPetMotion(index);
    final name = index < _motionFiles.length
        ? _motionFiles[index].name
        : '$index';
    _motionToast(
      result.ok ? '正在播放：$name' : (result.error ?? '播放失败'),
      error: !result.ok,
    );
  }

  Future<void> _renameMotion(PetMotionFile file) async {
    final controller = TextEditingController(text: file.name);
    final next = await showDialog<String>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: LunacColors.surface,
        title: Text(
          '重命名动作',
          style: TextStyle(color: LunacColors.text, fontSize: 13),
        ),
        content: TextField(
          controller: controller,
          autofocus: true,
          style: TextStyle(color: LunacColors.text, fontSize: 12),
          cursorColor: LunacColors.accent,
          decoration: InputDecoration(
            isDense: true,
            hintText: '动作名',
            hintStyle: TextStyle(color: LunacColors.textMuted, fontSize: 12),
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx),
            child: const Text('取消'),
          ),
          TextButton(
            onPressed: () => Navigator.pop(ctx, controller.text.trim()),
            child: const Text('确定'),
          ),
        ],
      ),
    );
    controller.dispose();
    if (!mounted || next == null || next.isEmpty || next == file.name) {
      return;
    }
    final model = _config.model.trim();
    final result = renamePetMotionFile(
      model,
      file.fileName,
      '$next.motion3.json',
    );
    if (result.error != null) {
      _motionToast(result.error!, error: true);
      return;
    }
    _refreshMotionFiles();
    _motionToast('已重命名为 $next');
  }

  Future<void> _deleteMotion(PetMotionFile file) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: LunacColors.surface,
        title: Text(
          '删除动作',
          style: TextStyle(color: LunacColors.text, fontSize: 13),
        ),
        content: Text(
          '删除「${file.name}」？此操作不可撤销。',
          style: TextStyle(color: LunacColors.textDim, fontSize: 12),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: const Text('取消'),
          ),
          TextButton(
            onPressed: () => Navigator.pop(ctx, true),
            child: Text('删除', style: TextStyle(color: LunacColors.danger)),
          ),
        ],
      ),
    );
    if (!mounted || confirmed != true) {
      return;
    }
    final result = deletePetMotionFile(_config.model.trim(), file.fileName);
    if (!result.ok) {
      _motionToast(result.error ?? '删除失败', error: true);
      return;
    }
    _refreshMotionFiles();
    _motionToast('已删除 ${file.name}');
  }

  /// 「新建动作」：向引擎要运行时参数表 → 生成 motion3 模板 → 落进模型目录。
  Future<void> _createMotion() async {
    final model = _config.model.trim();
    if (model.isEmpty) {
      _motionToast('先选一个 Live2D 模型。', error: true);
      return;
    }
    setState(() {
      _motionBusy = true;
      _motionMessage = null;
      _coreIds = null;
    });
    final request = requestPetCoreIds();
    Map<String, dynamic>? ids;
    if (request.ok) {
      final deadline = DateTime.now().add(const Duration(milliseconds: 2500));
      while (mounted && DateTime.now().isBefore(deadline)) {
        ids = _coreIds ?? _takeCoreIds(pollPetEvents());
        if (ids != null) {
          break;
        }
        await Future.delayed(const Duration(milliseconds: 80));
      }
    }
    if (!mounted) {
      return;
    }
    ids ??= _coreIds;
    if (ids == null) {
      setState(() => _motionBusy = false);
      _motionToast(
        request.error ?? '拿不到参数清单 —— 先「显示桌宠」一次，再来新建动作。',
        error: true,
      );
      return;
    }
    final physics = readPetPhysicsOutputs(model).ids.toSet();
    final template = buildPetMotionTemplate(
      coreIds: ids,
      physicsOutputs: physics,
      includePhysics: _config.motionTemplateIncludePhysics,
    );
    if (template == null) {
      setState(() => _motionBusy = false);
      _motionToast('拿不到参数清单 —— 先「显示桌宠」一次，再来新建动作。', error: true);
      return;
    }
    final name =
        'lunac_${DateTime.now().millisecondsSinceEpoch.toRadixString(36)}'
        '.motion3.json';
    final written = writePetMotionFile(model, name, template);
    if (!mounted) {
      return;
    }
    setState(() => _motionBusy = false);
    if (written.error != null) {
      _motionToast('生成失败：${written.error}', error: true);
      return;
    }
    final curves = _curveCount(template);
    _refreshMotionFiles();
    _motionToast('已生成 $name（$curves 条参数曲线），可交给 AI 编。');
  }

  Map<String, dynamic>? _takeCoreIds(List<PetEvent> events) {
    for (final event in events) {
      if (event.coreIds != null) {
        return event.coreIds;
      }
      if (event.config != null) {
        _config = event.config!;
      }
    }
    return null;
  }

  int _curveCount(String template) {
    try {
      final decoded = jsonDecode(template);
      final curves = decoded is Map ? decoded['Curves'] : null;
      return curves is List ? curves.length : 0;
    } catch (_) {
      return 0;
    }
  }

  void _toggleMotionPhysics() {
    final next = !_config.motionTemplateIncludePhysics;
    final saved = savePetConfig({'motionTemplateIncludePhysics': next});
    setState(() => _config = saved);
  }

  /// 「预设动作」页（v1 `data-pet-page="motion"`）：行 = 文件。
  Widget _buildMotionPage() {
    final model = _config.model.trim();
    return Container(
      margin: const EdgeInsets.only(top: 10),
      padding: const EdgeInsets.fromLTRB(10, 8, 10, 10),
      decoration: BoxDecoration(
        color: LunacColors.background,
        border: Border.all(color: LunacColors.border),
        borderRadius: BorderRadius.circular(8),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            '预设动作',
            style: TextStyle(
              color: LunacColors.text,
              fontSize: 12,
              fontWeight: FontWeight.w600,
            ),
          ),
          if (model.isEmpty)
            Padding(
              padding: const EdgeInsets.only(top: 6),
              child: Text(
                '先在「更换形象」里导入一个 Live2D 模型（*.model3.json），才能列出动作。',
                style: TextStyle(
                  color: LunacColors.textMuted,
                  fontSize: 11.5,
                  height: 1.45,
                ),
              ),
            )
          else ...[
            const SizedBox(height: 6),
            Row(
              children: [
                TextButton(
                  onPressed: _motionBusy ? null : _createMotion,
                  style: TextButton.styleFrom(
                    foregroundColor: LunacColors.text,
                    backgroundColor: LunacColors.surfaceHover,
                    side: BorderSide(color: LunacColors.divider),
                    padding: const EdgeInsets.symmetric(
                      horizontal: 10,
                      vertical: 4,
                    ),
                    minimumSize: Size.zero,
                    visualDensity: VisualDensity.compact,
                  ),
                  child: const Text(
                    '新建动作（motion3 模板）',
                    style: TextStyle(fontSize: 11),
                  ),
                ),
                if (_motionBusy) ...[
                  const SizedBox(width: 8),
                  SizedBox(
                    width: 12,
                    height: 12,
                    child: CircularProgressIndicator(
                      strokeWidth: 1.5,
                      color: LunacColors.accent,
                    ),
                  ),
                ],
              ],
            ),
            Row(
              children: [
                SizedBox(
                  width: 104,
                  child: Text(
                    '模板包含物理输出',
                    style: TextStyle(color: LunacColors.textDim, fontSize: 11),
                  ),
                ),
                TextButton(
                  onPressed: _toggleMotionPhysics,
                  style: TextButton.styleFrom(
                    foregroundColor: _config.motionTemplateIncludePhysics
                        ? LunacColors.text
                        : LunacColors.textDim,
                    backgroundColor: _config.motionTemplateIncludePhysics
                        ? LunacColors.surfaceHover
                        : LunacColors.surface,
                    side: BorderSide(
                      color: _config.motionTemplateIncludePhysics
                          ? LunacColors.divider
                          : LunacColors.border,
                    ),
                    padding: const EdgeInsets.symmetric(
                      horizontal: 14,
                      vertical: 4,
                    ),
                    minimumSize: Size.zero,
                    visualDensity: VisualDensity.compact,
                    shape: RoundedRectangleBorder(
                      borderRadius: BorderRadius.circular(999),
                    ),
                  ),
                  child: Text(
                    _config.motionTemplateIncludePhysics ? '开' : '关',
                    style: const TextStyle(fontSize: 11),
                  ),
                ),
              ],
            ),
            Padding(
              padding: const EdgeInsets.only(left: 104, top: 1),
              child: Text(
                '物理输出参数每帧被物理写值，模板里铺曲线无效；只有关掉物理想手动驱动头发 / 尾巴时才打开。',
                style: TextStyle(
                  color: LunacColors.textMuted,
                  fontSize: 10,
                  height: 1.35,
                ),
              ),
            ),
            const SizedBox(height: 6),
            if (_motionFiles.isEmpty)
              Text(
                '这个模型文件夹里还没有 *.motion3.json 动作。用上面的「新建动作」生成一个模板。',
                style: TextStyle(
                  color: LunacColors.textMuted,
                  fontSize: 11,
                  height: 1.4,
                ),
              )
            else
              for (var i = 0; i < _motionFiles.length; i++) _motionRow(i),
            if (_motionMessage != null)
              Padding(
                padding: const EdgeInsets.only(top: 6),
                child: Text(
                  _motionMessage!,
                  style: TextStyle(
                    color: _motionError
                        ? LunacColors.danger
                        : LunacColors.textMuted,
                    fontSize: 10.5,
                  ),
                ),
              ),
          ],
        ],
      ),
    );
  }

  /// 一行动作：序号 + 名字 + 播放 / 改名 / 删除（v1 行 = 文件）。
  Widget _motionRow(int index) {
    final file = _motionFiles[index];
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 2),
      child: Row(
        children: [
          SizedBox(
            width: 22,
            child: Text(
              '${index + 1}',
              style: TextStyle(color: LunacColors.textMuted, fontSize: 10.5),
            ),
          ),
          Expanded(
            child: Text(
              file.name,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(color: LunacColors.textDim, fontSize: 11),
            ),
          ),
          _motionAction('播放', () => _playMotion(index)),
          _motionAction('改名', () => _renameMotion(file)),
          _motionAction(
            '删除',
            () => _deleteMotion(file),
            color: LunacColors.danger,
          ),
        ],
      ),
    );
  }

  Widget _motionAction(String label, VoidCallback onTap, {Color? color}) {
    return TextButton(
      onPressed: onTap,
      style: TextButton.styleFrom(
        foregroundColor: color ?? LunacColors.textDim,
        padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 2),
        minimumSize: Size.zero,
        visualDensity: VisualDensity.compact,
      ),
      child: Text(label, style: const TextStyle(fontSize: 10.5)),
    );
  }

  // ── 热键页（v1 `data-pet-page="hotkey"`）──────────────────────────

  /// 重读模型的热键表（`lunac-live2d\lunac.hotkeys.json`）。
  void _refreshHotkeys() {
    final model = _config.model.trim();
    if (model.isEmpty) {
      setState(() => _hotkeys = const []);
      return;
    }
    final hotkeys = readPetHotkeys(model);
    setState(() {
      _hotkeys = hotkeys;
      _hotkeyMessage = null;
      _hotkeyError = false;
    });
  }

  void _hotkeyToast(String text, {bool error = false}) {
    if (!mounted) {
      return;
    }
    setState(() {
      _hotkeyMessage = text;
      _hotkeyError = error;
    });
  }

  String _newHotkeyId() {
    _hotkeySeq += 1;
    return '${DateTime.now().millisecondsSinceEpoch.toRadixString(36)}'
        '${_hotkeySeq.toRadixString(36)}';
  }

  /// 落盘并同步面板状态；成功时把落点路径写进页内提示（用户能看到「存到哪了」）。
  void _saveHotkeys(List<PetHotkey> next, String successText) {
    final model = _config.model.trim();
    if (model.isEmpty) {
      return;
    }
    final result = writePetHotkeys(model, next);
    if (result.error != null) {
      _hotkeyToast(result.error!, error: true);
      return;
    }
    setState(() => _hotkeys = next);
    _hotkeyToast('$successText；已存到 ${result.path ?? 'lunac.hotkeys.json'}');
  }

  /// 从模型文件夹导入表情热键（递归扫 `*.exp3.json`，已导入的按文件路径去重）。
  ///
  /// v1 还有一条「`.vtube.json` 作者热键」导入；v2 没有 vts 解析（如实标注在页内）。
  void _importHotkeys() {
    final model = _config.model.trim();
    if (model.isEmpty || _hotkeyBusy) {
      return;
    }
    setState(() => _hotkeyBusy = true);
    final result = readPetExp3Files(model);
    if (result.error != null) {
      setState(() => _hotkeyBusy = false);
      _hotkeyToast(result.error!, error: true);
      return;
    }
    final next = [..._hotkeys];
    final taken = {
      for (final hotkey in next)
        if (hotkey.action['file'] case final String file) file.toLowerCase(),
    };
    var added = 0;
    for (final file in result.files) {
      if (taken.contains(file.path.toLowerCase())) {
        continue;
      }
      next.add(
        PetHotkey(
          id: _newHotkeyId(),
          name: file.name,
          keys: defaultPetHotkeyKeys(next.length),
          action: {
            'type': 'expression',
            'target': file.name,
            'mode': 'set',
            'file': file.path,
          },
        ),
      );
      taken.add(file.path.toLowerCase());
      added += 1;
    }
    setState(() => _hotkeyBusy = false);
    if (added == 0) {
      _hotkeyToast(
        result.files.isEmpty
            ? '这个模型文件夹里没有 *.exp3.json 表情文件'
            : '没有新的表情可导入（当前 ${next.length} 条）',
      );
      return;
    }
    _saveHotkeys(next, '已导入 $added 条表情热键');
  }

  void _deleteHotkey(int index) {
    if (index < 0 || index >= _hotkeys.length) {
      return;
    }
    final next = [..._hotkeys]..removeAt(index);
    _saveHotkeys(next, '已删除 1 条热键');
  }

  void _clearHotkeys() {
    if (!_arm('hotkeys', note: '再点一次清空热键表（文件会被改写为空表）')) {
      return;
    }
    _saveHotkeys(const [], '热键表已清空');
  }

  Widget _buildHotkeyPage() {
    final model = _config.model.trim();
    return Container(
      margin: const EdgeInsets.only(top: 10),
      padding: const EdgeInsets.fromLTRB(10, 8, 10, 10),
      decoration: BoxDecoration(
        color: LunacColors.background,
        border: Border.all(color: LunacColors.border),
        borderRadius: BorderRadius.circular(8),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            '热键',
            style: TextStyle(
              color: LunacColors.text,
              fontSize: 12,
              fontWeight: FontWeight.w600,
            ),
          ),
          const SizedBox(height: 2),
          Text(
            '存在模型文件夹的 lunac-live2d\\lunac.hotkeys.json；按键只作 AI 触发标识，'
            '本版不做本地按键监听。导入范围 = 模型文件夹下递归的 *.exp3.json。',
            style: TextStyle(
              color: LunacColors.textMuted,
              fontSize: 10,
              height: 1.4,
            ),
          ),
          if (model.isEmpty)
            Padding(
              padding: const EdgeInsets.only(top: 6),
              child: Text(
                '先在「更换形象」里导入一个 Live2D 模型（*.model3.json），才能管理热键。',
                style: TextStyle(
                  color: LunacColors.textMuted,
                  fontSize: 11.5,
                  height: 1.45,
                ),
              ),
            )
          else ...[
            const SizedBox(height: 6),
            Row(
              children: [
                TextButton(
                  onPressed: _hotkeyBusy ? null : _importHotkeys,
                  style: TextButton.styleFrom(
                    foregroundColor: LunacColors.text,
                    backgroundColor: LunacColors.surfaceHover,
                    side: BorderSide(color: LunacColors.divider),
                    padding: const EdgeInsets.symmetric(
                      horizontal: 10,
                      vertical: 4,
                    ),
                    minimumSize: Size.zero,
                    visualDensity: VisualDensity.compact,
                  ),
                  child: const Text('从模型导入表情', style: TextStyle(fontSize: 11)),
                ),
                const SizedBox(width: 8),
                _motionAction(
                  '清空热键表',
                  _clearHotkeys,
                  color: _armedClear == 'hotkeys'
                      ? LunacColors.danger
                      : LunacColors.textMuted,
                ),
                const Spacer(),
                Text(
                  '${_hotkeys.length} 条',
                  style: TextStyle(
                    color: LunacColors.textMuted,
                    fontSize: 10.5,
                  ),
                ),
              ],
            ),
            const SizedBox(height: 4),
            if (_hotkeys.isEmpty)
              Text(
                '这个模型还没有热键。用上面的「从模型导入表情」把 *.exp3.json 收进来。',
                style: TextStyle(
                  color: LunacColors.textMuted,
                  fontSize: 11,
                  height: 1.4,
                ),
              )
            else
              for (var i = 0; i < _hotkeys.length; i++) _hotkeyRow(i),
            if (_hotkeyMessage != null)
              Padding(
                padding: const EdgeInsets.only(top: 6),
                child: Text(
                  _hotkeyMessage!,
                  style: TextStyle(
                    color: _hotkeyError
                        ? LunacColors.danger
                        : LunacColors.textMuted,
                    fontSize: 10.5,
                    height: 1.4,
                  ),
                ),
              ),
          ],
        ],
      ),
    );
  }

  /// 一行热键：名字 + 按键串 + 动作描述 + 删除。
  Widget _hotkeyRow(int index) {
    final hotkey = _hotkeys[index];
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 2),
      child: Row(
        children: [
          SizedBox(
            width: 22,
            child: Text(
              '${index + 1}',
              style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
            ),
          ),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  hotkey.name.isEmpty ? '（未命名）' : hotkey.name,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(color: LunacColors.text, fontSize: 11.5),
                ),
                Text(
                  '${hotkey.keys.isEmpty ? '（无按键）' : hotkey.keys} · '
                  '${hotkey.actionLabel}',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: LunacColors.textMuted,
                    fontSize: 10,
                    height: 1.35,
                  ),
                ),
              ],
            ),
          ),
          _motionAction(
            '删除',
            () => _deleteHotkey(index),
            color: LunacColors.danger,
          ),
        ],
      ),
    );
  }

  // ── 语音朗读页（v1 `data-pet-page="voice"`）────────────────────────

  void _refreshVoice() {
    final voice = readPetVoice();
    setState(() {
      _voice = voice;
      _voiceEngine = voice.engine;
      _voiceText.text = voice.referenceText;
      _voiceServer.text = voice.serverUrl;
      _voiceSteps.text = voice.sampleSteps.toString();
      _voiceSpeed.text = voice.speed.toStringAsFixed(1);
      _voiceVolume.text = '${(voice.volume * 100).round()}';
      if (voice.error != null) {
        _voiceMessage = voice.error;
        _voiceError = true;
      }
    });
  }

  void _voiceToast(String text, {bool error = false}) {
    if (!mounted) {
      return;
    }
    setState(() {
      _voiceMessage = text;
      _voiceError = error;
    });
  }

  /// 保存语音配置；成功时用 Rust 回传的**归一化后**状态回填控件（越界值会被夹回去）。
  void _saveVoice(Map<String, Object?> patch, String successText) {
    if (_voiceBusy) {
      return;
    }
    setState(() => _voiceBusy = true);
    final result = savePetVoice(patch);
    if (!mounted) {
      return;
    }
    setState(() => _voiceBusy = false);
    if (!result.ok) {
      _voiceToast(result.error ?? '保存语音配置失败', error: true);
      return;
    }
    final status = result.status;
    if (status != null) {
      setState(() {
        _voice = status;
        _voiceEngine = status.engine;
        _voiceText.text = status.referenceText;
        _voiceServer.text = status.serverUrl;
        _voiceSteps.text = status.sampleSteps.toString();
        _voiceSpeed.text = status.speed.toStringAsFixed(1);
        _voiceVolume.text = '${(status.volume * 100).round()}';
      });
    } else {
      _refreshVoice();
    }
    _voiceToast(successText);
  }

  void _toggleVoice(bool enabled) {
    _saveVoice({'enabled': enabled}, enabled ? '已开启语音朗读' : '已关闭语音朗读');
  }

  void _commitVoiceFields() {
    final speed = double.tryParse(_voiceSpeed.text.trim()) ?? _voice.speed;
    final volumePercent =
        int.tryParse(_voiceVolume.text.trim()) ?? (_voice.volume * 100).round();
    final steps = int.tryParse(_voiceSteps.text.trim()) ?? _voice.sampleSteps;
    _saveVoice({
      'engine': _voiceEngine,
      'serverUrl': _voiceServer.text.trim(),
      'sampleSteps': steps,
      'referenceText': _voiceText.text.trim(),
      'speed': speed,
      'volume': (volumePercent.clamp(0, 100)) / 100.0,
    }, '语音配置已保存');
  }

  /// 「试听」：先保存当前控件值（引擎 / 服务 / 步数 / 参考文本），再用当前音色读一句。
  void _previewVoice() {
    if (_voiceBusy) {
      return;
    }
    _commitVoiceFields();
    final result = speakPetVoice('你好呀，我今天也会好好陪着你。');
    if (!result.ok) {
      _voiceToast(result.error ?? '试听失败', error: true);
      return;
    }
    _voiceToast('正在合成并播放…');
  }

  /// 「选择参考音频」：导入后立刻保存路径（参考文本仍需用户填写）。
  void _pickVoiceAudio() {
    if (_voiceBusy) {
      return;
    }
    final path = pickPetVoiceAudio();
    if (path == null || path.isEmpty) {
      return;
    }
    _saveVoice({'referenceAudio': path}, '已导入参考音频');
  }

  Widget _buildVoicePage() {
    final voice = _voice;
    final speedHint =
        '${voice.speedMin.toStringAsFixed(1)} ~ ${voice.speedMax.toStringAsFixed(1)}';
    return Container(
      margin: const EdgeInsets.only(top: 10),
      padding: const EdgeInsets.fromLTRB(10, 8, 10, 10),
      decoration: BoxDecoration(
        color: LunacColors.background,
        border: Border.all(color: LunacColors.border),
        borderRadius: BorderRadius.circular(8),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            '语音朗读',
            style: TextStyle(
              color: LunacColors.text,
              fontSize: 12,
              fontWeight: FontWeight.w600,
            ),
          ),
          const SizedBox(height: 2),
          Text(
            'GPT-SoVITS（V5 / gsv.cpp）：先启动本机 gsv_server（默认 9880），'
            '再导入 3~10 秒参考录音 + 参考文本；试听和桌宠朗读都走这个服务。',
            style: TextStyle(
              color: LunacColors.textMuted,
              fontSize: 10,
              height: 1.4,
            ),
          ),
          if (voice.error != null)
            Padding(
              padding: const EdgeInsets.only(top: 6),
              child: Text(
                voice.error!,
                style: TextStyle(color: LunacColors.danger, fontSize: 11),
              ),
            )
          else ...[
            _switchRow(
              title: '语音朗读',
              subtitle: voice.enabled ? '已开启' : '已关闭',
              value: voice.enabled,
              onChanged: _voiceBusy ? (_) {} : _toggleVoice,
            ),
            const SizedBox(height: 6),
            Row(
              children: [
                SizedBox(
                  width: 64,
                  child: Text(
                    '引擎',
                    style: TextStyle(color: LunacColors.textDim, fontSize: 11),
                  ),
                ),
                _button(
                  'GPT-SoVITS',
                  () => setState(() => _voiceEngine = 'gpt_sovits'),
                  active: _voiceEngine == 'gpt_sovits',
                ),
                const SizedBox(width: 8),
                _button(
                  'ZipVoice（旧）',
                  () => setState(() => _voiceEngine = 'zipvoice'),
                  active: _voiceEngine == 'zipvoice',
                ),
              ],
            ),
            const SizedBox(height: 6),
            _voiceFieldRow(
              label: '服务',
              hint: 'http://127.0.0.1:9880',
              controller: _voiceServer,
              width: 240,
              keyboardType: TextInputType.url,
            ),
            _voiceFieldRow(
              label: '采样步数',
              hint: 'v5turbo 4 / v5dev 32',
              controller: _voiceSteps,
              width: 74,
            ),
            _voiceFieldRow(
              label: '语速',
              hint: speedHint,
              controller: _voiceSpeed,
              width: 74,
            ),
            _voiceFieldRow(
              label: '音量 %',
              hint: '0 ~ 100',
              controller: _voiceVolume,
              width: 74,
            ),
            const SizedBox(height: 4),
            Text(
              '参考文本（这段参考录音里说的那句话）',
              style: TextStyle(color: LunacColors.textDim, fontSize: 11),
            ),
            const SizedBox(height: 4),
            TextField(
              controller: _voiceText,
              enabled: !_voiceBusy,
              style: TextStyle(color: LunacColors.text, fontSize: 11.5),
              decoration: InputDecoration(
                isDense: true,
                hintText: '例：今天也要好好工作呀',
                hintStyle: TextStyle(
                  color: LunacColors.textMuted,
                  fontSize: 11.5,
                ),
                filled: true,
                fillColor: LunacColors.surface,
                contentPadding: const EdgeInsets.symmetric(
                  horizontal: 10,
                  vertical: 8,
                ),
                border: OutlineInputBorder(
                  borderRadius: BorderRadius.circular(6),
                  borderSide: BorderSide(color: LunacColors.border),
                ),
                enabledBorder: OutlineInputBorder(
                  borderRadius: BorderRadius.circular(6),
                  borderSide: BorderSide(color: LunacColors.border),
                ),
                focusedBorder: OutlineInputBorder(
                  borderRadius: BorderRadius.circular(6),
                  borderSide: BorderSide(color: LunacColors.accent),
                ),
              ),
            ),
            const SizedBox(height: 6),
            Row(
              children: [
                Expanded(
                  child: _infoLine(
                    '参考音色',
                    voice.referenceAudio.isEmpty
                        ? '未导入（3~10 秒）'
                        : voice.referenceAudio,
                  ),
                ),
                _button('选择音频', _pickVoiceAudio),
              ],
            ),
            if (voice.engine != 'gpt_sovits') _infoLine('资产目录', voice.assetDir),
            if (voice.missingAssets.isNotEmpty)
              _infoLine('缺失', voice.missingAssets.join(' / ')),
            if (voice.reasonText.isNotEmpty)
              Padding(
                padding: const EdgeInsets.only(top: 4),
                child: Text(
                  voice.reasonText,
                  style: TextStyle(
                    color:
                        voice.reason == 'assets_missing' ||
                            voice.reason == 'gsv_server_unreachable'
                        ? LunacColors.danger
                        : LunacColors.textMuted,
                    fontSize: 10,
                    height: 1.45,
                  ),
                ),
              ),
            const SizedBox(height: 6),
            Row(
              children: [
                _button(
                  _voiceBusy ? '保存中…' : '保存',
                  _commitVoiceFields,
                  primary: true,
                ),
                const SizedBox(width: 10),
                _button('试听', _previewVoice, active: voice.canSpeak),
                const SizedBox(width: 10),
                Expanded(
                  child: Text(
                    voice.canSpeak ? '服务已连通' : '服务未就绪',
                    style: TextStyle(
                      color: voice.canSpeak
                          ? LunacColors.success
                          : LunacColors.textMuted,
                      fontSize: 10,
                    ),
                  ),
                ),
              ],
            ),
            if (_voiceMessage != null)
              Padding(
                padding: const EdgeInsets.only(top: 6),
                child: Text(
                  _voiceMessage!,
                  style: TextStyle(
                    color: _voiceError
                        ? LunacColors.danger
                        : LunacColors.textMuted,
                    fontSize: 10.5,
                    height: 1.4,
                  ),
                ),
              ),
          ],
        ],
      ),
    );
  }

  Widget _voiceFieldRow({
    required String label,
    required String hint,
    required TextEditingController controller,
    required double width,
    TextInputType? keyboardType,
  }) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 4),
      child: Row(
        children: [
          SizedBox(
            width: 64,
            child: Text(
              label,
              style: TextStyle(color: LunacColors.textDim, fontSize: 11),
            ),
          ),
          SizedBox(
            width: width,
            child: TextField(
              controller: controller,
              enabled: !_voiceBusy,
              keyboardType:
                  keyboardType ??
                  const TextInputType.numberWithOptions(decimal: true),
              style: TextStyle(color: LunacColors.text, fontSize: 11),
              decoration: InputDecoration(
                isDense: true,
                hintText: hint,
                hintStyle: TextStyle(
                  color: LunacColors.textMuted,
                  fontSize: 10.5,
                ),
                filled: true,
                fillColor: LunacColors.surface,
                contentPadding: const EdgeInsets.symmetric(
                  horizontal: 8,
                  vertical: 7,
                ),
                border: OutlineInputBorder(
                  borderRadius: BorderRadius.circular(6),
                  borderSide: BorderSide(color: LunacColors.border),
                ),
                enabledBorder: OutlineInputBorder(
                  borderRadius: BorderRadius.circular(6),
                  borderSide: BorderSide(color: LunacColors.border),
                ),
                focusedBorder: OutlineInputBorder(
                  borderRadius: BorderRadius.circular(6),
                  borderSide: BorderSide(color: LunacColors.accent),
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _infoLine(String label, String value) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 2),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            width: 64,
            child: Text(
              label,
              style: TextStyle(color: LunacColors.textMuted, fontSize: 10.5),
            ),
          ),
          Expanded(
            child: Text(
              value.isEmpty ? '—' : value,
              style: TextStyle(
                color: LunacColors.textMuted,
                fontSize: 10,
                height: 1.4,
              ),
            ),
          ),
        ],
      ),
    );
  }

  /// 「模型调试」页（v1 `data-pet-page="debug"`）：表情按钮 + 常用参数滑条。
  Widget _buildDebugPage() {
    final model = _config.model.trim();
    final info = _modelInfo;
    final expressions = info?.expressionNames ?? const <String>[];
    return Container(
      margin: const EdgeInsets.only(top: 10),
      padding: const EdgeInsets.fromLTRB(10, 8, 10, 10),
      decoration: BoxDecoration(
        color: LunacColors.background,
        border: Border.all(color: LunacColors.border),
        borderRadius: BorderRadius.circular(8),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (model.isEmpty)
            Text(
              '先在「更换形象」里导入一个 Live2D 模型（*.model3.json），才能调试参数。',
              style: TextStyle(
                color: LunacColors.textMuted,
                fontSize: 11.5,
                height: 1.45,
              ),
            )
          else ...[
            Text(
              model,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(color: LunacColors.textDim, fontSize: 10.5),
            ),
            if (info != null && !info.ok)
              Padding(
                padding: const EdgeInsets.only(top: 4),
                child: Text(
                  '模型不可用：${info.reason}',
                  style: TextStyle(color: LunacColors.danger, fontSize: 10.5),
                ),
              ),
            const SizedBox(height: 10),
            Text(
              '表情',
              style: TextStyle(
                color: LunacColors.text,
                fontSize: 12,
                fontWeight: FontWeight.w600,
              ),
            ),
            const SizedBox(height: 4),
            if (expressions.isEmpty)
              Text(
                '该模型没有表情',
                style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
              )
            else
              Wrap(
                spacing: 6,
                runSpacing: 6,
                children: [
                  for (final name in expressions) _expressionChip(name),
                  _expressionChip('', reset: true),
                ],
              ),
            const SizedBox(height: 12),
            Text(
              '参数',
              style: TextStyle(
                color: LunacColors.text,
                fontSize: 12,
                fontWeight: FontWeight.w600,
              ),
            ),
            const SizedBox(height: 2),
            Text(
              '拖动即时生效、松手写盘；每个模型记住各自的值。',
              style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
            ),
            const SizedBox(height: 4),
            for (final param in _debugParams) _debugParamRow(param),
            if (!_status.open) ...[
              const SizedBox(height: 6),
              Text(
                '桌宠还没显示 —— 拖动会记住数值，点「显示桌宠」后才会看到效果。',
                style: TextStyle(
                  color: LunacColors.textMuted,
                  fontSize: 10,
                  height: 1.4,
                ),
              ),
            ],
          ],
          if (_tabMessage != null)
            Padding(
              padding: const EdgeInsets.only(top: 8),
              child: Text(
                _tabMessage!,
                style: TextStyle(
                  color: _tabMessageError
                      ? LunacColors.danger
                      : LunacColors.textMuted,
                  fontSize: 11,
                ),
              ),
            ),
        ],
      ),
    );
  }

  /// 一枚表情按钮（`reset` = 末尾的「还原」）。
  Widget _expressionChip(String name, {bool reset = false}) {
    final active = reset
        ? _activeExpression.isEmpty
        : _activeExpression == name;
    return TextButton(
      onPressed: () => _setExpression(name),
      style: TextButton.styleFrom(
        foregroundColor: active ? LunacColors.text : LunacColors.textDim,
        backgroundColor: active
            ? LunacColors.surfaceHover
            : LunacColors.surface,
        side: BorderSide(
          color: active ? LunacColors.divider : LunacColors.border,
        ),
        padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 4),
        minimumSize: Size.zero,
        visualDensity: VisualDensity.compact,
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(999)),
      ),
      child: Text(reset ? '还原' : name, style: const TextStyle(fontSize: 11)),
    );
  }

  /// 换表情（空串 = 还原）。落盘由 Rust 侧按模型路径记。
  void _setExpression(String name) {
    setState(() => _activeExpression = name);
    final result = sendPetDebug({'expression': name});
    if (!mounted) {
      return;
    }
    if (!result.ok) {
      _tabToast(result.error, error: true);
      return;
    }
    _tabToast(
      !result.open
          ? '桌宠窗没有打开 —— 表情已记住，显示后会生效'
          : (name.isEmpty ? '已还原表情' : '已切换表情：$name'),
    );
  }

  /// 钉一个参数值（拖动时 `persist:false`，松手写盘）。
  void _setParam(String id, double value, {bool persist = false}) {
    setState(() => _debugValues[id] = value);
    final result = sendPetDebug({
      'param': {'id': id, 'value': value},
      'persist': persist,
    });
    if (!mounted || result.ok) {
      return;
    }
    _tabToast(result.error, error: true);
  }

  /// 一行参数：中文名 + 滑条 + 数值（区间与 v1 `DEBUG_PARAMS` 一致）。
  Widget _debugParamRow(
    ({String id, double min, double max, String label}) param,
  ) {
    final value = (_debugValues[param.id] ?? 0).clamp(param.min, param.max);
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 1),
      child: Row(
        children: [
          SizedBox(
            width: 64,
            child: Text(
              param.label,
              style: TextStyle(color: LunacColors.textDim, fontSize: 11),
            ),
          ),
          Expanded(
            child: SliderTheme(
              data: SliderTheme.of(context).copyWith(
                trackHeight: 2,
                thumbShape: const RoundSliderThumbShape(enabledThumbRadius: 5),
                overlayShape: SliderComponentShape.noOverlay,
                activeTrackColor: LunacColors.accent,
                inactiveTrackColor: LunacColors.border,
                thumbColor: LunacColors.accent,
              ),
              child: SizedBox(
                height: 20,
                child: Slider(
                  value: value,
                  min: param.min,
                  max: param.max,
                  divisions: (param.max - param.min).round(),
                  onChanged: (next) => _setParam(param.id, next),
                  onChangeEnd: (next) =>
                      _setParam(param.id, next, persist: true),
                ),
              ),
            ),
          ),
          SizedBox(
            width: 38,
            child: Text(
              value.toStringAsFixed(0),
              textAlign: TextAlign.right,
              style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
            ),
          ),
        ],
      ),
    );
  }

  /// 「桌宠人格」页（v1 `data-pet-page="persona"`）：核心人格 + 三份列表。
  Widget _buildPersonaPage() {
    final persona = _persona;
    final maxChars = persona?.maxChars ?? 8000;
    final personaArmed = _armedClear == 'persona';
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Container(
          margin: const EdgeInsets.only(top: 10),
          padding: const EdgeInsets.fromLTRB(10, 8, 10, 10),
          decoration: BoxDecoration(
            color: LunacColors.background,
            border: Border.all(color: LunacColors.border),
            borderRadius: BorderRadius.circular(8),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Text(
                '核心人格',
                style: TextStyle(
                  color: LunacColors.text,
                  fontSize: 12,
                  fontWeight: FontWeight.w600,
                ),
              ),
              const SizedBox(height: 3),
              Text(
                '只作用于桌宠发起的对话，不影响普通聊天。',
                style: TextStyle(
                  color: LunacColors.textMuted,
                  fontSize: 11,
                  height: 1.4,
                ),
              ),
              const SizedBox(height: 8),
              TextField(
                controller: _personaText,
                minLines: 5,
                maxLines: 8,
                maxLength: maxChars,
                onChanged: (_) => setState(() {}),
                style: TextStyle(
                  color: LunacColors.text,
                  fontSize: 12,
                  height: 1.5,
                ),
                cursorColor: LunacColors.accent,
                decoration: InputDecoration(
                  isDense: true,
                  counterText: '',
                  hintText: '写一段桌宠的人格设定…',
                  hintStyle: TextStyle(
                    color: LunacColors.textMuted,
                    fontSize: 11.5,
                  ),
                  filled: true,
                  fillColor: LunacColors.surface,
                  contentPadding: const EdgeInsets.all(8),
                  border: OutlineInputBorder(
                    borderRadius: BorderRadius.circular(6),
                    borderSide: BorderSide(color: LunacColors.border),
                  ),
                  enabledBorder: OutlineInputBorder(
                    borderRadius: BorderRadius.circular(6),
                    borderSide: BorderSide(color: LunacColors.border),
                  ),
                  focusedBorder: OutlineInputBorder(
                    borderRadius: BorderRadius.circular(6),
                    borderSide: BorderSide(color: LunacColors.accent),
                  ),
                ),
              ),
              const SizedBox(height: 4),
              Align(
                alignment: Alignment.centerRight,
                child: Text(
                  '${_personaText.text.runes.length} / $maxChars',
                  style: TextStyle(
                    color: LunacColors.textMuted,
                    fontSize: 10.5,
                  ),
                ),
              ),
              const SizedBox(height: 6),
              Wrap(
                spacing: 8,
                runSpacing: 8,
                children: [
                  _button('保存', _savePersona, primary: true),
                  _button(
                    personaArmed ? '再点一次清空（不可恢复）' : '清空',
                    _clearPersona,
                    danger: personaArmed,
                  ),
                  _button('立即重启 AI', _restartAi),
                ],
              ),
              if (_tabMessage != null)
                Padding(
                  padding: const EdgeInsets.only(top: 6),
                  child: Text(
                    _tabMessage!,
                    style: TextStyle(
                      color: _tabMessageError
                          ? LunacColors.danger
                          : LunacColors.textMuted,
                      fontSize: 11,
                    ),
                  ),
                ),
            ],
          ),
        ),
        _buildMemorySection(
          title: '事实记忆',
          desc: '关于你的事实：桌宠记住的你的信息。',
          list: _memoryList,
          kind: PetMemoryKind.memory,
        ),
        _buildMemorySection(
          title: '成长笔记',
          desc: '桌宠对自己的观察：它自己写下的成长记录。',
          list: _growthList,
          kind: PetMemoryKind.growth,
        ),
        _buildMemorySection(
          title: '人格建议',
          desc: '桌宠提议的人格修订。点「接受」会把这条并入核心人格。',
          list: _suggestList,
          kind: PetMemoryKind.suggestions,
          allowAccept: true,
          clearArmedNote: '再点一次清空建议（只是丢弃建议，不改核心人格）',
        ),
      ],
    );
  }

  /// 一份 bullet 列表（事实记忆 / 成长笔记 / 人格建议共用）。
  Widget _buildMemorySection({
    required String title,
    required String desc,
    required PetMemoryList? list,
    required PetMemoryKind kind,
    bool allowAccept = false,
    String clearArmedNote = '再点一次清空（不可恢复）',
  }) {
    final entries = list?.entries ?? const <String>[];
    final armed = _armedClear == kind.name;
    return Container(
      margin: const EdgeInsets.only(top: 12),
      padding: const EdgeInsets.fromLTRB(10, 8, 10, 10),
      decoration: BoxDecoration(
        color: LunacColors.background,
        border: Border.all(color: LunacColors.border),
        borderRadius: BorderRadius.circular(8),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            title,
            style: TextStyle(
              color: LunacColors.text,
              fontSize: 12,
              fontWeight: FontWeight.w600,
            ),
          ),
          const SizedBox(height: 3),
          Text(
            desc,
            style: TextStyle(
              color: LunacColors.textMuted,
              fontSize: 11,
              height: 1.4,
            ),
          ),
          if (list == null)
            Padding(
              padding: const EdgeInsets.only(top: 6),
              child: Text(
                '不可用（rust 桥接未加载）',
                style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
              ),
            )
          else if (entries.isEmpty)
            Padding(
              padding: const EdgeInsets.only(top: 6),
              child: Text(
                '暂无内容',
                style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
              ),
            )
          else
            for (final entry in entries)
              Container(
                margin: const EdgeInsets.only(top: 6),
                padding: const EdgeInsets.fromLTRB(8, 5, 2, 5),
                decoration: BoxDecoration(
                  color: LunacColors.surface,
                  borderRadius: BorderRadius.circular(6),
                ),
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Expanded(
                      child: Padding(
                        padding: const EdgeInsets.only(top: 3, right: 6),
                        child: SelectableText(
                          entry,
                          style: TextStyle(
                            color: LunacColors.textDim,
                            fontSize: 11.5,
                            height: 1.45,
                          ),
                        ),
                      ),
                    ),
                    if (allowAccept)
                      TextButton(
                        onPressed: () => _acceptSuggestion(entry),
                        style: TextButton.styleFrom(
                          foregroundColor: LunacColors.accent,
                          visualDensity: VisualDensity.compact,
                          padding: const EdgeInsets.symmetric(horizontal: 6),
                        ),
                        child: const Text('接受', style: TextStyle(fontSize: 11)),
                      ),
                    IconButton(
                      tooltip: '删除这一条',
                      icon: const Icon(Icons.close, size: 14),
                      color: LunacColors.textMuted,
                      visualDensity: VisualDensity.compact,
                      constraints: const BoxConstraints.tightFor(
                        width: 24,
                        height: 24,
                      ),
                      padding: EdgeInsets.zero,
                      onPressed: () => _deleteEntry(kind, entry),
                    ),
                  ],
                ),
              ),
          if (list != null && entries.isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(top: 8),
              child: Align(
                alignment: Alignment.centerRight,
                child: TextButton(
                  onPressed: () => _clearList(kind, clearArmedNote),
                  style: TextButton.styleFrom(
                    foregroundColor: armed
                        ? LunacColors.danger
                        : LunacColors.textDim,
                    visualDensity: VisualDensity.compact,
                  ),
                  child: Text(
                    armed ? clearArmedNote : '清空',
                    style: const TextStyle(fontSize: 11),
                  ),
                ),
              ),
            ),
        ],
      ),
    );
  }

  /// v2 追加的窗口动作（v1 控制台没有这几项，放在提示下方，不占首行）。
  Widget _buildExtras() {
    return Padding(
      padding: const EdgeInsets.only(top: 14),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            '窗口',
            style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
          ),
          _switchRow(
            title: '置顶',
            subtitle: '始终浮在其他窗口之上（v1 的桌宠窗恒为置顶）',
            value: _config.topmost,
            onChanged: _toggleTopmost,
          ),
          const SizedBox(height: 8),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: [
              _button('重置位置', _resetPosition),
              _button('退出桌宠', _quit, danger: true),
            ],
          ),
        ],
      ),
    );
  }

  /// 开关行（不用 `SwitchListTile`：它要求最近的 Material 祖先，而面板外框是
  /// `Container` 的 DecoratedBox —— 会触发 Flutter 的「ink 不可见」断言）。
  Widget _switchRow({
    required String title,
    required String subtitle,
    required bool value,
    required ValueChanged<bool> onChanged,
  }) {
    return Padding(
      padding: const EdgeInsets.only(top: 6),
      child: Row(
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  title,
                  style: TextStyle(color: LunacColors.text, fontSize: 12.5),
                ),
                const SizedBox(height: 2),
                Text(
                  subtitle,
                  style: TextStyle(
                    color: LunacColors.textMuted,
                    fontSize: 11.5,
                    height: 1.45,
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(width: 10),
          Switch(value: value, onChanged: onChanged),
        ],
      ),
    );
  }

  Widget _buildMessage() {
    return Padding(
      padding: const EdgeInsets.only(top: 12),
      child: Text(
        _message!,
        style: TextStyle(
          color: _messageError ? LunacColors.danger : LunacColors.textMuted,
          fontSize: 12,
          height: 1.5,
        ),
      ),
    );
  }
}
