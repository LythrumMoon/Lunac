// 设置面板（v1 `app/src/plugins/builtin/settings.ts` 的 Flutter 版）。
//
// 版式自 M5v 起按 v1 当前形态对齐：左侧分类抽屉（宽 30%）+ 右侧单分类内容，
// 分类与分区标题逐字取 v1 i18n（常规 / 风格 / AI / 搜索 / 插件；没有「关于」——
// 版本与更新检查在 v1 里就在「常规」分区），分区标题是主题色浅底 + 左侧 3px 色条，
// 行是扁平 `label + 控件` 的一行（不再是 v2 早期的卡片块）。
//
// 边界（docs/flutter-migration.md §一）：
//   · 所有读写都走平台薄层（`platform/settings.dart` / `platform/agent.dart` /
//     `platform/index_status.dart` / `platform/hotkey.dart`），本文件只做界面与交互，
//     不出现 FFI；唯一的直接联网是「测试连通」（v1 也有这个按钮）。
//   · 风格分区：点主题包立即切换（运行时色板 + config/appearance.json）；自定义配色
//     用 v1 同款取色器（hex 输入 + 饱和度-明度方块 + 色相条 + 8 个预设色 + 跟随主题），
//     背景图可选图 / 清空并调四个滑块；AI 分区是 v1 精简后的四项
//     （提供商 / 模型 / 接口地址 / API 密钥），vision 恒开。
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:isolate';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../platform/agent.dart';
import '../platform/ai_provider.dart';
import '../platform/appearance.dart';
import '../platform/curl_import.dart';
import '../platform/app_window.dart';
import '../platform/auto_start.dart';
import '../platform/cron.dart';
import '../platform/hotkey.dart';
import '../platform/index_status.dart';
import '../platform/mcp_config.dart';
import '../platform/memory.dart';
import '../platform/plugin_kv.dart';
import '../platform/plugin_market.dart';
import '../platform/plugin_toolchain.dart';
import '../platform/plugin_window.dart';
import '../platform/settings.dart';
import '../platform/shell_actions.dart';
import '../platform/skills_admin.dart';
import '../platform/updater.dart';
import '../platform/usage.dart';
import '../theme.dart';
import 'chat_model.dart';

/// 设置面板：搜索栏右侧圆钮 / 「设置」结果项打开。
///
/// 与详细搜索大界面同高（640 档），Esc / 左上角返回关闭。
class SettingsPanel extends StatefulWidget {
  const SettingsPanel({
    super.key,
    required this.onClose,
    this.onEngineChanged,
    this.onDrag,
  });

  /// 关闭面板（Esc / 返回按钮）。
  final VoidCallback onClose;

  /// 默认引擎切换成功（主界面据此重算 Web 搜索行）。
  final VoidCallback? onEngineChanged;

  /// 标题栏区域的拖动 = 移动窗口。
  ///
  /// 设置面板独占整窗时搜索栏（原本的唯一拖拽区）已隐藏，所以必须由面板标题栏
  /// 接过这条通道；null 时走平台默认的 `startWindowDrag`（单测可注入以断言命中）。
  final Future<void> Function()? onDrag;

  @override
  State<SettingsPanel> createState() => _SettingsPanelState();
}

/// 设置侧边栏的分类：**逐字取 v1 `i18n.ts` 的 `settings.sidebar_*`**
/// （常规 / 风格 / AI / 搜索 / 插件）。v1 只有这五栏，没有「关于」——
/// 版本与更新检查在 v1 里就摆在「常规」分区，v2 自 M5v 起同样并回「常规」。
const List<({String id, String label})> _settingsCategories = [
  (id: 'general', label: '常规'),
  (id: 'appearance', label: '风格'),
  (id: 'ai', label: 'AI'),
  (id: 'search', label: '搜索'),
  (id: 'plugins', label: '插件'),
];

/// 取色器的预设色板（逐字取 v1 `settings.ts::colorPickerHtml`）。
const List<String> _appearancePresets = [
  '#cfc8bb',
  '#3a7bd5',
  '#5b9a68',
  '#c9a227',
  '#c05555',
  '#8e6fc0',
  '#3f9a9a',
  '#d07aa0',
];

class _SettingsPanelState extends State<SettingsPanel> {
  late String _engine = webEngineName();

  /// 下拉里选中的引擎（还没点「保存」）；null = 与 _engine 一致。
  /// v1 的搜索分区就是「选择 + 保存」，不是即点即生效。
  String? _pendingEngine;
  String? _engineMessage;
  bool _engineError = false;

  final String? _dataDir = dataDirectory();

  /// 当前分类（点侧边栏切换；面板重开回到「通用」，与 v1 的
  /// `activeSettingsCategory` 缺省值一致）。
  String _category = 'general';

  /// 面板自己的焦点节点：录制热键时必须保证按键先到面板（否则会被页面级 Focus 截走）。
  late final FocusNode _panelFocus = FocusNode(
    debugLabel: 'settings-panel',
    onKeyEvent: _onKey,
  );

  // ── 通用：唤醒热键（录制 → Rust 换注册 → 写 config/hotkey.json）──
  String _hotkey = readHotkeyCombo();
  bool _recordingHotkey = false;
  String? _hotkeyMessage;
  bool _hotkeyError = false;

  // ── 通用：开机自启（Rust 侧注册表 + 计划任务双机制，可能弹 UAC）──
  AutoStartInfo? _autoStart = readAutoStart();
  bool _autoStartBusy = false;
  String? _autoStartMessage;
  bool _autoStartError = false;

  // ── 关于：更新检查 + 下载 / 静默安装 ──
  UpdateCheck? _update;
  bool _checkingUpdate = false;
  // ── 常规：自动更新（v1 2026-10-05 合并后的单开关，默认开）──
  bool _updateAuto = readUpdateConfig().enabled;
  String? _updateAutoMessage;
  bool _updateAutoError = false;

  // ── 常规：定时任务（v1 阶段 1：只读列表 + 立即运行；定义由 agent 侧写）──
  CronSnapshot _cron = readCronJobs();
  String? _cronBusyId;
  String? _cronMessage;
  bool _cronMessageError = false;

  /// 二次确认中的 job（「无人值守可写」必须点两次；3 秒后自动复位）。
  String? _cronWriteConfirmId;
  Timer? _cronConfirmTimer;

  // ── AI：技能（v1「设置 · AI · 技能」组）：`<exe 根>/skills/<key>/SKILL.md` ──
  //
  // 只做「列出 / 新建 / 编辑 / 删除 + 重启生效」；key 白名单与大小上限在 Rust 侧。
  SkillsSnapshot _skills = const SkillsSnapshot();
  bool _skillsLoaded = false;
  String? _skillMessage;
  bool _skillMessageError = false;

  // ── AI：技能级记忆（§待办 3-28.10 ①）：只读列表 + 预览 + 清空 ──
  SkillMemoryList _skillMemories = const SkillMemoryList();
  bool _skillMemoriesLoaded = false;

  // ── AI：「精选」索引（§待办 3-28.9 ①）：随包哈希钉住的 6 个官方技能 ──
  CuratedSnapshot _curated = const CuratedSnapshot();
  bool _curatedLoaded = false;
  String? _curatedBusyKey;

  // ── AI：工具 (MCP)（v1 `#sp-ai` 第三组）：远端服务器配置文件的唯一入口 ──
  String? _mcpMessage;
  bool _mcpMessageError = false;

  // ── AI：用量与成本（v1 `buildUsageCostSection`：打开价表 / 更新价格 / 候选确认）──
  PricingState _pricing = readPricingState();
  String? _costMessage;
  bool _costError = false;

  // ── 关于：安装流程状态（下载进度 / 二次确认 / 失败原因）──
  bool _installing = false;
  double? _installProgress;
  String? _installMessage;
  bool _installError = false;
  bool _confirmInstall = false;
  Timer? _confirmInstallTimer;

  // ── 风格：主题包 + 自定义配色（config/appearance.json）──
  final List<ThemePackage>? _themes = readThemes();
  AppearanceConfig _appearance = readAppearanceConfig();

  /// 当前生效的主题 id（点泡泡框即写盘 + 应用）。
  late String _appearanceTheme = _appearance.themeId;
  String? _appearanceMessage;
  bool _appearanceError = false;

  late final TextEditingController _baseColor = TextEditingController(
    text: _appearance.baseColor,
  );
  late final TextEditingController _btnLineColor = TextEditingController(
    text: _appearance.btnLineColor,
  );
  late final TextEditingController _btnBgColor = TextEditingController(
    text: _appearance.btnBgColor,
  );
  late final TextEditingController _overlayColor = TextEditingController(
    text: _appearance.overlayColor,
  );

  // 「自定义 ▾」面板的展开状态（v1 在会话内保留，不落盘）。
  bool _bgPanelOpen = false;
  bool _basePanelOpen = false;
  bool _btnPanelOpen = false;
  bool _textPanelOpen = false;
  bool _otherPanelOpen = false;

  /// 目前展开色板（SV 方块 + 色相条 + 预设）的那个颜色槽位（null = 都收起）。
  String? _pickerOpen;

  /// 每个槽位上次拖动色相条选中的色相 —— 颜色被拖成全白/全黑时色相仍能保留
  /// （v1 取色器把 HSV 存在实例里，这里按槽位记一份等价状态）。
  final Map<String, double> _pickerHue = {};

  FileIndexStatus? _indexStatus;
  Timer? _indexTimer;
  Timer? _cronTimer;

  /// 已经点过「重建索引」、等 Rust 侧 `scanning` 接管。
  bool _rebuilding = false;

  // ── 插件市场（M5m）：已装清单 + 远端索引 + 安装 / 卸载 ──
  bool _marketLoaded = false;
  bool _marketLoading = false;
  MarketInstalled? _marketInstalled;
  MarketIndex? _marketIndex;
  String? _marketMessage;
  bool _marketError = false;

  /// 正在安装 / 卸载的那一条（同一时刻只跑一个，按钮显示处理中）。
  String? _marketBusyId;

  /// 安装进度（`lunac_market_install_state` 的 500ms 轮询快照）。
  ///
  /// v2 没有 v1 的插件事件总线，所以用轮询代替 `plugin-install-progress` 事件 ——
  /// 语义一致（暂停 / 继续 / 断点续传都看这份状态）。
  MarketInstallState _marketState = const MarketInstallState();
  Timer? _marketPoll;

  /// 磁盘上还没下完的断点（重启后也能看到「继续下载 (n%)」）。
  List<MarketPendingDownload> _marketPending = const [];

  // ── AI 模型（v1 config/ai.json 的精简四项）──
  final Map<String, dynamic> _aiConfig = readAiConfig();
  late String _aiProvider = _initialProvider();
  late final TextEditingController _aiUrl = TextEditingController(
    text: _aiString('url'),
  );
  late final TextEditingController _aiModel = TextEditingController(
    text: _aiString('model'),
  );
  late final TextEditingController _aiKey = TextEditingController(
    text: _aiString('key'),
  );
  bool _aiKeyVisible = false;
  String? _aiMessage;
  bool _aiError = false;
  bool _aiTesting = false;

  // ── AI 下拉的自定义 / 隐藏名单（v1 localStorage 的四个键）──
  // v1：`lunac-custom-providers` / `lunac-hidden-providers` / `lunac-custom-models`
  // / `lunac-hidden-models`；v2 没有 WebView，落 `config/ai.json` 的同名 camelCase 键。
  late final List<String> _customProviders = _aiStringList('customProviders');
  late final List<String> _hiddenProviders = _aiStringList('hiddenProviders');
  late final Map<String, List<String>> _customModels = _aiModelMap(
    'customModels',
  );
  late final Map<String, List<String>> _hiddenModels = _aiModelMap(
    'hiddenModels',
  );

  List<String> _aiStringList(String key) {
    final value = _aiConfig[key];
    if (value is! List) {
      return <String>[];
    }
    return [
      for (final item in value)
        if (item is String && item.trim().isNotEmpty) item.trim(),
    ];
  }

  Map<String, List<String>> _aiModelMap(String key) {
    final value = _aiConfig[key];
    if (value is! Map) {
      return <String, List<String>>{};
    }
    return {
      for (final entry in value.entries)
        if (entry.key is String && entry.value is List)
          entry.key as String: [
            for (final item in entry.value as List)
              if (item is String && item.trim().isNotEmpty) item.trim(),
          ],
    };
  }

  /// 把四个名单落盘（只覆盖这四个键，`provider` / `key` 等由「保存」按钮管）。
  void _persistAiLists() {
    saveAiConfig({
      'customProviders': _customProviders,
      'hiddenProviders': _hiddenProviders,
      'customModels': _customModels,
      'hiddenModels': _hiddenModels,
    });
  }

  /// 供应商下拉的选项：内置预设（去掉隐藏）+ 自定义供应商 + 当前值兜底。
  List<_AiOption> get _providerOptions {
    final out = <_AiOption>[];
    for (final preset in aiProviderPresets) {
      if (_hiddenProviders.contains(preset.key)) {
        continue;
      }
      out.add(_AiOption(value: preset.key, label: preset.label));
    }
    for (final key in _customProviders) {
      if (_hiddenProviders.contains(key) ||
          aiProviderPresets.any((preset) => preset.key == key)) {
        continue;
      }
      out.add(_AiOption(value: key, label: key));
    }
    if (!out.any((option) => option.value == _aiProvider)) {
      out.add(_AiOption(value: _aiProvider, label: _aiProvider));
    }
    return out;
  }

  /// 模型下拉的选项：该供应商的常用模型（去掉隐藏）+ 自定义模型 + 当前值兜底。
  List<_AiOption> get _modelOptions {
    final out = <_AiOption>[];
    final hidden = _hiddenModels[_aiProvider] ?? const <String>[];
    for (final model in aiModelSuggestions[_aiProvider] ?? const <String>[]) {
      if (hidden.contains(model)) {
        continue;
      }
      out.add(_AiOption(value: model, label: model));
    }
    for (final model in _customModels[_aiProvider] ?? const <String>[]) {
      if (hidden.contains(model) ||
          out.any((option) => option.value == model)) {
        continue;
      }
      out.add(_AiOption(value: model, label: model));
    }
    final current = _aiModel.text.trim();
    if (current.isNotEmpty && !out.any((option) => option.value == current)) {
      out.add(_AiOption(value: current, label: current));
    }
    return out;
  }

  /// × 删除：自定义项直接摘掉，内置项进隐藏名单（v1 同口径）。
  void _deleteAiOption({required bool provider, required String value}) {
    setState(() {
      if (provider) {
        if (aiProviderPresets.any((preset) => preset.key == value)) {
          if (!_hiddenProviders.contains(value)) {
            _hiddenProviders.add(value);
          }
        } else {
          _customProviders.remove(value);
        }
      } else {
        final presetModels =
            aiModelSuggestions[_aiProvider] ?? const <String>[];
        if (presetModels.contains(value)) {
          final hidden = _hiddenModels.putIfAbsent(
            _aiProvider,
            () => <String>[],
          );
          if (!hidden.contains(value)) {
            hidden.add(value);
          }
        } else {
          _customModels[_aiProvider]?.remove(value);
        }
      }
    });
    _persistAiLists();
    // 删掉的是当前选中项 ⇒ 回落到第一个剩余项（v1 `onDelete` 的同一段逻辑）。
    if (provider && value == _aiProvider) {
      final options = _providerOptions;
      if (options.isNotEmpty) {
        _selectAiProvider(options.first.value);
      }
    } else if (!provider && value == _aiModel.text.trim()) {
      final options = _modelOptions;
      setState(
        () => _aiModel.text = options.isEmpty ? '' : options.first.value,
      );
    }
  }

  /// `__custom__` 新增：写入自定义名单并选中（命中内置项时从隐藏名单里恢复）。
  void _addCustomAiOption({required bool provider, required String raw}) {
    final value = raw.trim();
    if (value.isEmpty) {
      return;
    }
    setState(() {
      if (provider) {
        if (aiProviderPresets.any((preset) => preset.key == value)) {
          _hiddenProviders.remove(value);
        } else if (!_customProviders.contains(value)) {
          _customProviders.add(value);
        }
      } else {
        final list = _customModels.putIfAbsent(_aiProvider, () => <String>[]);
        if (!list.contains(value)) {
          list.add(value);
        }
        _hiddenModels[_aiProvider]?.remove(value);
        _aiModel.text = value;
      }
    });
    _persistAiLists();
    if (provider) {
      _selectAiProvider(value);
    }
  }

  String _aiString(String key) {
    final value = _aiConfig[key];
    return value is String ? value : '';
  }

  String _initialProvider() {
    final provider = _aiString('provider');
    if (provider.isNotEmpty) {
      return provider;
    }
    return 'deepseek';
  }

  @override
  void initState() {
    super.initState();
    _refreshIndex();
    _refreshCron();
    // 面板自己拿焦点：录制热键 / Esc 都靠它先收到按键（页面级 Focus 是祖先，收不到子级按键）。
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) {
        _panelFocus.requestFocus();
      }
    });
    // 与详细搜索同频率：2 秒一轮，面板关闭即停。
    _indexTimer = Timer.periodic(
      const Duration(seconds: 2),
      (_) => _refreshIndex(),
    );
    // 定时任务的运行态由宿主调度线程写；5 秒轮询一次就够。
    _cronTimer = Timer.periodic(
      const Duration(seconds: 5),
      (_) => _refreshCron(),
    );
  }

  @override
  void dispose() {
    _indexTimer?.cancel();
    _cronTimer?.cancel();
    _marketPoll?.cancel();
    _confirmInstallTimer?.cancel();
    _cronConfirmTimer?.cancel();
    _panelFocus.dispose();
    _aiUrl.dispose();
    _aiModel.dispose();
    _aiKey.dispose();
    _baseColor.dispose();
    _btnLineColor.dispose();
    _btnBgColor.dispose();
    super.dispose();
  }

  void _refreshIndex() {
    final status = readFileIndexStatus();
    if (!mounted) {
      return;
    }
    setState(() {
      _indexStatus = status;
      if (status != null && !status.scanning) {
        _rebuilding = false;
      }
    });
  }

  // ── 定时任务（v1 阶段 1 口径：只读列表 + 立即运行 + 逐 job 可写授权）──

  void _refreshCron() {
    final snapshot = readCronJobs();
    if (!mounted) {
      return;
    }
    setState(() => _cron = snapshot);
  }

  // ── AI：技能（v1「设置 · AI · 技能」组）──────────────────────────────

  /// 读技能列表（只在第一次进 AI 页与点「刷新」时读，不轮询）。
  void _refreshSkills() {
    final snapshot = listSkills();
    if (!mounted) {
      return;
    }
    setState(() {
      _skills = snapshot;
      _skillsLoaded = true;
    });
    _refreshSkillMemories();
    _refreshCurated();
  }

  /// 读「精选」索引（§待办 3-28.9 ①）：随包清单 + 与本地已装技能的哈希比对。
  void _refreshCurated() {
    final snapshot = readCuratedSkills();
    if (!mounted) {
      return;
    }
    setState(() {
      _curated = snapshot;
      _curatedLoaded = true;
    });
  }

  /// 从「精选」安装 / 恢复：被安全闸或权限 diff 拦下时用一句话确认后再放行。
  ///
  /// 判定不在前端：`permissionBlocked` / `scanBlocked` 都是 Rust 给的结论，
  /// 这里只负责把理由念给用户听、再按原样重发一次。
  Future<void> _installCurated(CuratedSkill entry) async {
    if (_curatedBusyKey != null) {
      return;
    }
    setState(() => _curatedBusyKey = entry.key);
    try {
      var result = installCuratedSkill(
        key: entry.key,
        overwrite: !entry.missing,
      );
      if (!result.ok &&
          (result.permissionBlocked || result.scanBlocked) &&
          mounted) {
        final confirmed = await showDialog<bool>(
          context: context,
          builder: (ctx) => AlertDialog(
            backgroundColor: LunacColors.background,
            title: Text(
              '安装「${entry.key}」需要确认',
              style: TextStyle(color: LunacColors.text, fontSize: 13.5),
            ),
            content: SizedBox(
              width: 480,
              child: Text(
                '${result.error}\n\n'
                '${result.permissionBlocked ? '这会让技能拿到更多工具。' : '检查结果已标记为危险。'}'
                '确认后才会落盘。',
                style: TextStyle(
                  color: LunacColors.textDim,
                  fontSize: 12,
                  height: 1.45,
                ),
              ),
            ),
            actions: [
              TextButton(
                onPressed: () => Navigator.of(ctx).pop(false),
                child: Text('取消', style: TextStyle(color: LunacColors.textDim)),
              ),
              TextButton(
                onPressed: () => Navigator.of(ctx).pop(true),
                child: Text(
                  '仍要安装',
                  style: TextStyle(color: LunacColors.danger),
                ),
              ),
            ],
          ),
        );
        if (confirmed != true) {
          return;
        }
        result = installCuratedSkill(
          key: entry.key,
          overwrite: !entry.missing,
          acceptScan: result.scanBlocked,
          acceptPermissions: result.permissionBlocked,
        );
      }
      if (!mounted) {
        return;
      }
      setState(() {
        _skillMessage = result.ok
            ? '已从精选安装「${entry.key}」（${result.hashShort}）'
            : '安装「${entry.key}」失败：${result.error}';
        _skillMessageError = !result.ok;
      });
      _refreshSkills();
    } finally {
      if (mounted) {
        setState(() => _curatedBusyKey = null);
      }
    }
  }

  /// 读技能级记忆清单（§待办 3-28.10 ①）：只列有内容的 `skills/<key>/MEMORY.md`。
  void _refreshSkillMemories() {
    final snapshot = readSkillMemories();
    if (!mounted) {
      return;
    }
    setState(() {
      _skillMemories = snapshot;
      _skillMemoriesLoaded = true;
    });
  }

  /// 预览某个技能的记忆；对话框里可以直接清空（破坏性操作二次确认）。
  Future<void> _showSkillMemory(String key) async {
    final text = readSkillMemory(key);
    if (!mounted) {
      return;
    }
    await showDialog<void>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: LunacColors.background,
        title: Text(
          '技能记忆：$key',
          style: TextStyle(color: LunacColors.text, fontSize: 13.5),
        ),
        content: SizedBox(
          width: 560,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                text.ok ? text.text : '读取失败：${text.error}',
                style: TextStyle(
                  color: text.ok ? LunacColors.textDim : LunacColors.danger,
                  fontSize: 11.5,
                  height: 1.45,
                ),
              ),
              const SizedBox(height: 8),
              Text(
                '这些笔记只注入「这个技能 fork 出来的子代理」，不进主对话前缀。',
                style: TextStyle(color: LunacColors.textMuted, fontSize: 10.5),
              ),
            ],
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(),
            child: Text('关闭', style: TextStyle(color: LunacColors.textDim)),
          ),
          TextButton(
            onPressed: () {
              Navigator.of(ctx).pop();
              unawaited(_clearSkillMemory(key));
            },
            child: Text('清空', style: TextStyle(color: LunacColors.danger)),
          ),
        ],
      ),
    );
  }

  Future<void> _clearSkillMemory(String key) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: LunacColors.background,
        title: Text(
          '清空技能记忆「$key」？',
          style: TextStyle(color: LunacColors.text, fontSize: 13.5),
        ),
        content: Text(
          '会删掉这个技能的全部私有笔记（不可恢复）。技能本身与全局长期记忆都不受影响。',
          style: TextStyle(color: LunacColors.textDim, fontSize: 12),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(false),
            child: Text('取消', style: TextStyle(color: LunacColors.textDim)),
          ),
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(true),
            child: Text('清空', style: TextStyle(color: LunacColors.danger)),
          ),
        ],
      ),
    );
    if (confirmed != true) {
      return;
    }
    final ok = clearSkillMemory(key);
    if (!mounted) {
      return;
    }
    setState(() {
      _skillMessage = ok ? '已清空技能「$key」的记忆' : '清空失败（$key）';
      _skillMessageError = !ok;
    });
    _refreshSkillMemories();
  }

  Future<void> _createSkill() async {
    const template =
        '---\nname: 新技能\ndescription: 一句话说明什么时候该用这个技能\n---\n\n'
        '（正文：给模型的操作步骤 / 约束。需要长资料时放到同目录的其它文件里，'
        '正文用相对路径引用。）\n';
    await _showSkillEditor(key: '', content: template, keyEditable: true);
  }

  /// 从 raw URL 导入（v1 的「安装技能」）：只收 https；同名要二次确认才覆盖。
  ///
  /// 下载走 `Isolate.run`（Rust 侧是阻塞 reqwest，最长 20s），否则整个设置面板会卡住。
  Future<void> _installSkillFromUrl() async {
    final keyController = TextEditingController();
    final urlController = TextEditingController();
    final hashController = TextEditingController();
    final submitted = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: LunacColors.background,
        title: Text(
          '从 URL 安装技能',
          style: TextStyle(color: LunacColors.text, fontSize: 13.5),
        ),
        content: SizedBox(
          width: 520,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              TextField(
                controller: keyController,
                style: TextStyle(color: LunacColors.text, fontSize: 12.5),
                decoration: InputDecoration(
                  isDense: true,
                  labelText: 'key（目录名）',
                  hintText: '只允许字母 / 数字 / _ / -，例如 code-review',
                  hintStyle: TextStyle(
                    color: LunacColors.textMuted,
                    fontSize: 11,
                  ),
                ),
              ),
              const SizedBox(height: 10),
              TextField(
                controller: urlController,
                style: TextStyle(color: LunacColors.text, fontSize: 12.5),
                decoration: InputDecoration(
                  isDense: true,
                  labelText: 'SKILL.md 的 https 地址',
                  hintText: 'https://raw.githubusercontent.com/…/SKILL.md',
                  hintStyle: TextStyle(
                    color: LunacColors.textMuted,
                    fontSize: 11,
                  ),
                ),
              ),
              const SizedBox(height: 10),
              TextField(
                controller: hashController,
                style: TextStyle(color: LunacColors.text, fontSize: 12.5),
                decoration: InputDecoration(
                  isDense: true,
                  labelText: 'sha256（可选）',
                  hintText: '清单里钉的 64 位十六进制；填了就必须完全一致',
                  hintStyle: TextStyle(
                    color: LunacColors.textMuted,
                    fontSize: 11,
                  ),
                ),
              ),
              const SizedBox(height: 10),
              Text(
                '只收 https 地址，单份正文上限 128 KiB。若同名技能已存在，会再问一次是否覆盖；'
                '升级若扩大 allowed-tools 范围，还会再确认一次。',
                style: TextStyle(
                  color: LunacColors.textMuted,
                  fontSize: 11,
                  height: 1.5,
                ),
              ),
            ],
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(false),
            child: const Text('取消'),
          ),
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(true),
            child: const Text('下载并安装'),
          ),
        ],
      ),
    );
    final key = keyController.text.trim();
    final url = urlController.text.trim();
    final hash = hashController.text.trim();
    keyController.dispose();
    urlController.dispose();
    hashController.dispose();
    if (submitted != true || !mounted) {
      return;
    }
    setState(() {
      _skillMessageError = false;
      _skillMessage = '正在下载 $url …';
    });
    var result = await Isolate.run(
      () => installSkillFromUrl(key: key, url: url, sha256: hash),
    );
    if (!mounted) {
      return;
    }
    // 用户确认过「覆盖同名技能」之后才置真：安全闸那一步重发时要把它带上，
    // 否则用户点完「仍要安装」会再撞一次「同名已存在」。
    var overwriteOk = false;
    if (!result.ok && result.exists) {
      final overwrite = await showDialog<bool>(
        context: context,
        builder: (ctx) => AlertDialog(
          backgroundColor: LunacColors.background,
          title: Text(
            '覆盖技能「$key」',
            style: TextStyle(color: LunacColors.text, fontSize: 13.5),
          ),
          content: Text(
            'skills\\$key 已存在，导入会整个替换它的 SKILL.md。'
            '同目录下的其它资源文件不受影响。',
            style: TextStyle(
              color: LunacColors.textDim,
              fontSize: 12,
              height: 1.5,
            ),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.of(ctx).pop(false),
              child: const Text('取消'),
            ),
            TextButton(
              onPressed: () => Navigator.of(ctx).pop(true),
              child: const Text('覆盖'),
            ),
          ],
        ),
      );
      if (overwrite != true || !mounted) {
        setState(() => _skillMessage = null);
        return;
      }
      overwriteOk = true;
      result = await Isolate.run(
        () => installSkillFromUrl(
          key: key,
          url: url,
          overwrite: true,
          sha256: hash,
        ),
      );
      if (!mounted) {
        return;
      }
    }
    // 安全闸（§待办 3-28.9）：命中 danger 时 Rust 不落盘，先让用户看检查结果。
    var scanAccepted = false;
    if (!result.ok && result.scanBlocked) {
      final proceed = await _confirmRiskyScan(key, result.scan);
      if (proceed != true || !mounted) {
        setState(() => _skillMessage = null);
        return;
      }
      scanAccepted = true;
      result = await Isolate.run(
        () => installSkillFromUrl(
          key: key,
          url: url,
          overwrite: overwriteOk,
          acceptScan: true,
          sha256: hash,
        ),
      );
      if (!mounted) {
        return;
      }
    }
    // 权限 diff（§待办 3-28.9 ③）：升级扩大了 `allowed-tools` 必须再确认一次。
    if (!result.ok && result.permissionBlocked) {
      final proceed = await _confirmPermissionExpand(key, result.permissions);
      if (proceed != true || !mounted) {
        setState(() => _skillMessage = null);
        return;
      }
      result = await Isolate.run(
        () => installSkillFromUrl(
          key: key,
          url: url,
          overwrite: overwriteOk,
          acceptScan: scanAccepted,
          sha256: hash,
          acceptPermissions: true,
        ),
      );
      if (!mounted) {
        return;
      }
    }
    setState(() {
      _skillMessageError = !result.ok;
      final scanNote = result.scan == null || result.scan!.clean
          ? ''
          : '，安全检查：${result.scan!.label}';
      final hashNote = result.sha256.isEmpty
          ? ''
          : '，sha256 ${result.hashShort}…';
      _skillMessage = result.ok
          ? '已导入 ${result.name.isEmpty ? result.key : result.name}'
                '（${result.bytes} 字节$scanNote$hashNote，重启 AI 后生效）'
          : result.error;
    });
    if (result.ok) {
      _refreshSkills();
    }
  }

  /// 权限扩大的二次确认：把「新增了哪些工具 / 变成不限制」说清楚（§待办 3-28.9 ③）。
  Future<bool?> _confirmPermissionExpand(
    String key,
    SkillPermissions? permissions,
  ) {
    final added = permissions?.added ?? const <String>[];
    final detail = permissions == null
        ? '这次升级扩大了工具权限。'
        : permissions.unrestricted
        ? '升级后不再限制工具名（任何工具都能用）。'
        : '新增工具：${added.join('、')}。';
    final kept = permissions?.removed ?? const <String>[];
    return showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: LunacColors.background,
        title: Text(
          '升级会扩大技能「$key」的权限',
          style: TextStyle(color: LunacColors.danger, fontSize: 13.5),
        ),
        content: SizedBox(
          width: 460,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                detail,
                style: TextStyle(
                  color: LunacColors.text,
                  fontSize: 12,
                  height: 1.6,
                ),
              ),
              if (kept.isNotEmpty) ...[
                const SizedBox(height: 8),
                Text(
                  '不再列出：${kept.join('、')}',
                  style: TextStyle(
                    color: LunacColors.textMuted,
                    fontSize: 11.5,
                    height: 1.6,
                  ),
                ),
              ],
              const SizedBox(height: 10),
              Text(
                '权限只影响这个技能在子代理里能调用哪些工具。只有你确认过才继续。',
                style: TextStyle(
                  color: LunacColors.textMuted,
                  fontSize: 11,
                  height: 1.5,
                ),
              ),
            ],
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(false),
            child: const Text('取消'),
          ),
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(true),
            child: const Text('仍要升级'),
          ),
        ],
      ),
    );
  }

  /// 危险技能的二次确认：把命中的行 / 原文 / 原因列出来，用户点了「仍要安装」才继续。
  Future<bool?> _confirmRiskyScan(String key, SkillScan? scan) {
    final findings = scan?.findings ?? const <SkillFinding>[];
    return showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: LunacColors.background,
        title: Text(
          '这个技能包含危险操作',
          style: TextStyle(color: LunacColors.danger, fontSize: 13.5),
        ),
        content: SizedBox(
          width: 520,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Text(
                '技能正文会被模型照着做。下面这些行被判定为危险，'
                '除非你确认这是你要的内容，否则不要安装。',
                style: TextStyle(
                  color: LunacColors.textDim,
                  fontSize: 12,
                  height: 1.5,
                ),
              ),
              const SizedBox(height: 8),
              ConstrainedBox(
                constraints: const BoxConstraints(maxHeight: 260),
                child: ListView(
                  shrinkWrap: true,
                  children: [
                    for (final finding in findings)
                      Padding(
                        padding: const EdgeInsets.only(bottom: 6),
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(
                              '${finding.line > 0 ? '第 ${finding.line} 行 · ' : ''}'
                              '${finding.why}',
                              style: TextStyle(
                                color: finding.danger
                                    ? LunacColors.danger
                                    : LunacColors.text,
                                fontSize: 11.5,
                                height: 1.4,
                              ),
                            ),
                            if (finding.excerpt.isNotEmpty)
                              Text(
                                finding.excerpt,
                                maxLines: 2,
                                overflow: TextOverflow.ellipsis,
                                style: TextStyle(
                                  color: LunacColors.textMuted,
                                  fontSize: 10.5,
                                  fontFamily: 'Consolas',
                                  fontFamilyFallback: const [
                                    'Courier New',
                                    'monospace',
                                  ],
                                ),
                              ),
                          ],
                        ),
                      ),
                  ],
                ),
              ),
            ],
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(false),
            child: const Text('取消（不安装）'),
          ),
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(true),
            child: Text(
              '仍要安装「$key」',
              style: TextStyle(color: LunacColors.danger),
            ),
          ),
        ],
      ),
    );
  }

  Future<void> _editSkill(SkillEntry skill) async {
    final read = readSkill(skill.key);
    if (!read.ok) {
      setState(() {
        _skillMessageError = true;
        _skillMessage = read.error.isEmpty ? '读不了 ${skill.key}' : read.error;
      });
      return;
    }
    await _showSkillEditor(
      key: skill.key,
      content: read.content,
      keyEditable: false,
    );
  }

  Future<void> _showSkillEditor({
    required String key,
    required String content,
    required bool keyEditable,
  }) async {
    final keyController = TextEditingController(text: key);
    final contentController = TextEditingController(text: content);
    // 检查结果（局部状态）：只在对话框里显示，不落盘、不进 `_skillMessage` ——
    // 用户可能改了正文再检查，面板那条消息会被下一次保存覆盖。
    SkillScan? scan;
    final saved = await showDialog<bool>(
      context: context,
      builder: (ctx) => StatefulBuilder(
        builder: (ctx, setDialogState) => AlertDialog(
          backgroundColor: LunacColors.background,
          title: Text(
            keyEditable ? '新建技能' : '编辑技能：$key',
            style: TextStyle(color: LunacColors.text, fontSize: 13.5),
          ),
          content: SizedBox(
            width: 520,
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                TextField(
                  controller: keyController,
                  enabled: keyEditable,
                  style: TextStyle(color: LunacColors.text, fontSize: 12.5),
                  decoration: InputDecoration(
                    isDense: true,
                    labelText: 'key（目录名）',
                    hintText: '只允许字母 / 数字 / _ / -，例如 code-review',
                    hintStyle: TextStyle(
                      color: LunacColors.textMuted,
                      fontSize: 11,
                    ),
                  ),
                ),
                const SizedBox(height: 10),
                TextField(
                  controller: contentController,
                  minLines: 12,
                  maxLines: 18,
                  style: TextStyle(
                    color: LunacColors.text,
                    fontSize: 12,
                    fontFamily: 'Consolas',
                    fontFamilyFallback: const ['Courier New', 'monospace'],
                  ),
                  decoration: InputDecoration(
                    isDense: true,
                    border: const OutlineInputBorder(),
                    labelText: 'SKILL.md',
                  ),
                ),
                if (scan != null)
                  Padding(
                    padding: const EdgeInsets.only(top: 8),
                    child: Text(
                      '安全检查：${scan!.label}'
                      '${scan!.findings.isEmpty ? '' : ' —— ${scan!.summary}'}',
                      style: TextStyle(
                        color: scan!.danger
                            ? LunacColors.danger
                            : LunacColors.textMuted,
                        fontSize: 11,
                        height: 1.4,
                      ),
                    ),
                  ),
              ],
            ),
          ),
          actions: [
            TextButton(
              onPressed: () => setDialogState(() {
                scan = scanSkill(contentController.text);
              }),
              child: const Text('检查'),
            ),
            TextButton(
              onPressed: () => Navigator.of(ctx).pop(false),
              child: const Text('取消'),
            ),
            TextButton(
              onPressed: () => Navigator.of(ctx).pop(true),
              child: const Text('保存'),
            ),
          ],
        ),
      ),
    );
    final result = saved == true
        ? saveSkill(
            key: keyController.text.trim(),
            content: contentController.text,
          )
        : null;
    keyController.dispose();
    contentController.dispose();
    if (result == null || !mounted) {
      return;
    }
    setState(() {
      _skillMessageError = !result.ok;
      _skillMessage = result.ok ? '已保存 ${result.key}' : result.error;
    });
    if (result.ok) {
      _refreshSkills();
    }
  }

  Future<void> _deleteSkill(SkillEntry skill) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: LunacColors.background,
        title: Text(
          '删除技能「${skill.title}」',
          style: TextStyle(color: LunacColors.text, fontSize: 13.5),
        ),
        content: Text(
          '整个目录 skills/${skill.key} 会被删掉（含里面的资源文件）。此操作不可撤销。',
          style: TextStyle(
            color: LunacColors.textDim,
            fontSize: 12,
            height: 1.5,
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(false),
            child: const Text('取消'),
          ),
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(true),
            child: Text('删除', style: TextStyle(color: LunacColors.danger)),
          ),
        ],
      ),
    );
    if (confirmed != true) {
      return;
    }
    final result = deleteSkill(skill.key);
    if (!mounted) {
      return;
    }
    setState(() {
      _skillMessageError = !result.ok;
      _skillMessage = result.ok ? '已删除 ${skill.key}' : result.error;
    });
    if (result.ok) {
      _refreshSkills();
    }
  }

  /// 技能改完必须重启 agent 才重读（`LUNAC_SKILLS_DIR` 只在进程启动时读一次）。
  void _restartAgentForSkills() {
    stopAgent();
    final reason = startAgent();
    if (!mounted) {
      return;
    }
    setState(() {
      _skillMessageError = reason != null;
      _skillMessage = reason ?? 'AI 已重启：技能清单已重读（正在进行的回合已结束）';
    });
  }

  Future<void> _runCronJob(CronJob job) async {
    setState(() {
      _cronBusyId = job.id;
      _cronMessage = null;
    });
    final result = await runCronJobNow(job.id);
    if (!mounted) {
      return;
    }
    setState(() {
      _cronBusyId = null;
      _cronMessageError = !result.ok;
      _cronMessage = result.ok
          ? '这一轮已跑完（${job.scheduleText}）'
          : '运行失败：${result.error}';
    });
    _refreshCron();
  }

  /// 启用 / 停用一条定时任务（§待办 3-28.16 的前端入口）。
  ///
  /// 重新启用时 Rust 侧会同时清掉「自动停用」与连续失败计数并重排下一次 —— 界面上
  /// 那句「已启用」不是空话，刷新后能看到新的「下次」时间。
  Future<void> _toggleCronEnabled(CronJob job) async {
    final enabled = !job.enabled;
    setState(() {
      _cronBusyId = job.id;
      _cronMessage = null;
    });
    final result = await setCronEnabled(job.id, enabled);
    if (!mounted) {
      return;
    }
    setState(() {
      _cronBusyId = null;
      _cronMessageError = !result.ok;
      _cronMessage = result.ok
          ? (enabled ? '已启用定时任务；下次运行时刻已重排' : '已停用；调度器不再触发这一条')
          : '切换失败：${result.error}';
    });
    _refreshCron();
  }

  void _toggleCronWrite(CronJob job) {
    if (_cronWriteConfirmId != job.id) {
      _cronConfirmTimer?.cancel();
      setState(() {
        _cronWriteConfirmId = job.id;
        _cronMessageError = false;
        _cronMessage = job.writeAccess
            ? '再点一次「关」：定时任务将不再能改文件'
            : '再点一次「开」：定时任务可在无人值守下改文件（自行承担风险）';
      });
      _cronConfirmTimer = Timer(const Duration(seconds: 3), () {
        if (mounted) {
          setState(() {
            _cronWriteConfirmId = null;
            _cronMessage = null;
          });
        }
      });
      return;
    }
    _cronConfirmTimer?.cancel();
    final allow = !job.writeAccess;
    setState(() {
      _cronWriteConfirmId = null;
      _cronMessage = null;
    });
    unawaited(
      setCronWriteAccess(job.id, allow).then((result) {
        if (!mounted) {
          return;
        }
        setState(() {
          _cronMessageError = !result.ok;
          _cronMessage = result.ok
              ? (allow ? '已允许无人值守写入' : '已关闭无人值守写入')
              : '切换失败：${result.error}';
        });
        _refreshCron();
      }),
    );
  }

  String _cronStatusText(CronJob job) {
    if (job.autoDisabled) {
      return '连续失败已自动停用';
    }
    return switch (job.lastStatus) {
      'ok' => '上次成功',
      'failed' => '上次失败',
      'skipped_busy' => '上次因忙碌跳过',
      'skipped_missed' => '上次错过（超过 10 分钟未补跑）',
      _ => '从未运行',
    };
  }

  /// 下拉改选中项：只更新界面，等用户点「保存」才落盘（v1 同口径）。
  void _selectEngine(String name) {
    setState(() {
      _pendingEngine = name == _engine ? null : name;
      _engineMessage = null;
    });
  }

  void _saveEngine() {
    final target = _pendingEngine ?? _engine;
    if (target != _engine) {
      if (!setWebEngine(target)) {
        setState(() {
          _engineError = true;
          _engineMessage = '保存失败（rust 桥接未加载）';
        });
        return;
      }
      setState(() {
        _engine = target;
        _pendingEngine = null;
      });
      widget.onEngineChanged?.call();
    }
    setState(() {
      _engineError = false;
      _engineMessage = '已保存';
    });
  }

  void _rebuild() {
    if (!rebuildFileIndex()) {
      return;
    }
    setState(() => _rebuilding = true);
  }

  // ── 插件市场 ────────────────────────────────────────────────────

  /// 拉已装清单（本机扫目录）+ 远端索引（走网络 → 后台 isolate）。
  Future<void> _loadMarket() async {
    if (_marketLoading) {
      return;
    }
    setState(() {
      _marketLoaded = true;
      _marketLoading = true;
      _marketError = false;
      _marketMessage = null;
    });
    // 已装清单是本机扫描（同步）：先画出来，别让「已装」跟着网络一起等。
    final installed = readInstalledPlugins();
    if (mounted) {
      setState(() => _marketInstalled = installed);
    }
    final raw = await Isolate.run(fetchMarketIndexRaw);
    if (!mounted) {
      return;
    }
    setState(() {
      _marketLoading = false;
      _marketInstalled = installed;
      _marketIndex = MarketIndex.fromRaw(raw);
    });
    // 断点是本机扫目录（毫秒级）：进页面就能看到「继续下载 (37%)」。
    _refreshMarketDownloads();
  }

  /// 读一次断点清单 + 安装进度（都不阻塞）。
  void _refreshMarketDownloads() {
    final pending = readMarketPendingDownloads();
    final state = readMarketInstallState();
    if (!mounted) {
      return;
    }
    setState(() {
      _marketPending = pending ?? const [];
      _marketState = state;
    });
    if (state.active && _marketPoll == null) {
      _startMarketPoll();
    }
  }

  /// 安装期间每 500ms 读一次进度快照（v1 是事件推送，v2 用轮询）。
  void _startMarketPoll() {
    _marketPoll?.cancel();
    _marketPoll = Timer.periodic(const Duration(milliseconds: 500), (_) {
      if (!mounted) {
        _marketPoll?.cancel();
        _marketPoll = null;
        return;
      }
      final state = readMarketInstallState();
      final pending = readMarketPendingDownloads();
      setState(() {
        _marketState = state;
        if (pending != null) {
          _marketPending = pending;
        }
      });
      // 安装结束（成功 / 失败 / 取消）就停表：再转下去只是白耗 CPU。
      if (!state.active) {
        _marketPoll?.cancel();
        _marketPoll = null;
      }
    });
  }

  /// 下载并安装 / 更新一条市场插件（地址与 sha256 都由索引给出）。
  Future<void> _installMarket(MarketIndexEntry entry) async {
    if (_marketBusyId != null) {
      return;
    }
    setState(() {
      _marketBusyId = entry.id;
      _marketError = false;
      _marketMessage = '正在下载并安装「${entry.name}」…';
    });
    // 断点存在时这次是**续传**，不是从 0 开始（`.part` 在 temp/downloads）。
    _startMarketPoll();
    final raw = await Isolate.run(
      () => installMarketPluginRaw(entry.url, entry.sha256),
    );
    if (!mounted) {
      return;
    }
    final result = MarketActionResult.fromRaw(raw, entry.id);
    setState(() {
      _marketBusyId = null;
      _marketError = !result.ok;
      _marketMessage = result.ok
          ? '已安装「${entry.name}」${entry.version.isEmpty ? '' : ' ${entry.version}'}'
                '（写入 Modules 目录；当前版本不加载第三方插件）'
          : result.error;
    });
    _refreshMarketDownloads();
    if (result.ok) {
      await _loadMarket();
    }
  }

  /// 暂停 / 继续当前这次安装（暂停时下载连接断开、原地等；继续时重发 `Range` 接上）。
  void _toggleMarketPause() {
    final next = !_marketState.paused;
    pauseMarketInstall(next);
    setState(() {
      _marketState = MarketInstallState(
        busy: _marketState.busy,
        active: _marketState.active,
        kind: _marketState.kind,
        id: _marketState.id,
        url: _marketState.url,
        phase: _marketState.phase,
        downloaded: _marketState.downloaded,
        total: _marketState.total,
        paused: next,
        resumable: _marketState.resumable,
        index: _marketState.index,
        count: _marketState.count,
      );
      _marketMessage = next ? '已暂停：点「继续」接着下（断点已保存）' : '继续下载…';
      _marketError = false;
    });
  }

  /// 取消当前这次依赖下载（断点文件一起删掉，下次从头来）。
  void _cancelMarketInstall() {
    cancelMarketDependency();
    setState(() {
      _marketMessage = '已取消下载（断点已删除）';
      _marketError = false;
    });
    _refreshMarketDownloads();
  }

  /// 给已装插件补装清单里的依赖（下载 + 校验 + 落盘全在 Rust 侧）。
  Future<void> _installMarketDeps(String id, String name) async {
    if (_marketBusyId != null) {
      return;
    }
    setState(() {
      _marketBusyId = id;
      _marketError = false;
      _marketMessage = '正在给「$name」补装依赖…';
    });
    _startMarketPoll();
    final raw = await Isolate.run(() => installMarketDependenciesRaw(id));
    if (!mounted) {
      return;
    }
    final result = MarketActionResult.fromRaw(raw, id);
    setState(() {
      _marketBusyId = null;
      _marketError = !result.ok;
      _marketMessage = result.ok ? '已给「$name」补装依赖' : result.error;
    });
    _refreshMarketDownloads();
  }

  /// 这一条有没有磁盘上的断点（有就把按钮换成「继续下载 (n%)」）。
  MarketPendingDownload? _pendingForRow(_MarketRow row) {
    for (final p in _marketPending) {
      if (p.id.isNotEmpty && p.id == row.id) {
        return p;
      }
      if (p.id.isEmpty && row.url.isNotEmpty && p.url == row.url) {
        return p;
      }
    }
    return null;
  }

  /// 断点的人话名字：有 id 用 id，否则用 URL 尾段；有百分比时一起给。
  String _pendingName(MarketPendingDownload p) {
    final who = p.id.isEmpty ? urlTail(p.url) : p.id;
    final pct = percentText(p.fraction);
    return pct.isEmpty ? who : '$who ($pct)';
  }

  /// 打开插件运行时控制台（sidecar：起进程 / 信任卡 / 调用 / 事件）。
  ///
  /// 走独立插件窗（v1 的插件面板也是独立窗）；单测 / 无宿主时如实说明。
  Future<void> _openPluginRuntime(String pluginId) async {
    final result = await openPluginWindow('plugin-console', query: pluginId);
    if (!mounted) {
      return;
    }
    setState(() {
      _marketError = result != PluginOpenResult.opened;
      _marketMessage = switch (result) {
        PluginOpenResult.opened => '已打开「$pluginId」的运行时控制台',
        PluginOpenResult.limitReached => '插件窗最多同时开两个，先关掉一个再试',
        PluginOpenResult.unavailable => '没有插件窗口宿主（单测 / Linux 预览下不可用）',
      };
    });
  }

  /// 插件存储（v1 `plugin_kv` 的等价物，§M5z-补69）：只读清单 + 清空。
  ///
  /// 键值由插件自己写（宿主侧 API 见 `platform/plugin_kv.dart`）；这里给用户
  /// 「它存了多少、能不能一键清空」—— 不提供编辑单个值的入口（那是插件的事）。
  Future<void> _openPluginStorage(String pluginId, String pluginName) async {
    final snapshot = listPluginKv(pluginId);
    final cleared = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: LunacColors.background,
        title: Text(
          '插件存储：$pluginName',
          style: TextStyle(color: LunacColors.text, fontSize: 13.5),
        ),
        content: SizedBox(
          width: 460,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Text(
                snapshot.ok
                    ? '${snapshot.entries.length} 个键 · ${snapshot.bytes} 字节'
                          '（上限 256 KiB，文件 ModuleData\\plugins\\$pluginId\\kv.json）'
                    : (snapshot.error.isEmpty ? '读不了插件存储' : snapshot.error),
                style: TextStyle(
                  color: snapshot.ok
                      ? LunacColors.textMuted
                      : LunacColors.danger,
                  fontSize: 11,
                  height: 1.5,
                ),
              ),
              if (snapshot.ok && snapshot.entries.isNotEmpty) ...[
                const SizedBox(height: 8),
                ConstrainedBox(
                  constraints: const BoxConstraints(maxHeight: 240),
                  child: ListView(
                    shrinkWrap: true,
                    children: [
                      for (final entry in snapshot.entries)
                        Padding(
                          padding: const EdgeInsets.symmetric(vertical: 1),
                          child: Row(
                            children: [
                              Expanded(
                                child: Text(
                                  entry.key,
                                  maxLines: 1,
                                  overflow: TextOverflow.ellipsis,
                                  style: TextStyle(
                                    color: LunacColors.text,
                                    fontSize: 11.5,
                                  ),
                                ),
                              ),
                              Text(
                                '${entry.bytes} B',
                                style: TextStyle(
                                  color: LunacColors.textMuted,
                                  fontSize: 10,
                                ),
                              ),
                            ],
                          ),
                        ),
                    ],
                  ),
                ),
              ],
            ],
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(false),
            child: const Text('关闭'),
          ),
          TextButton(
            onPressed: snapshot.ok && snapshot.entries.isNotEmpty
                ? () => Navigator.of(ctx).pop(true)
                : null,
            child: Text('清空', style: TextStyle(color: LunacColors.danger)),
          ),
        ],
      ),
    );
    if (cleared != true || !mounted) {
      return;
    }
    final result = clearPluginKv(pluginId);
    if (!mounted) {
      return;
    }
    setState(() {
      _marketError = !result.ok;
      _marketMessage = result.ok ? '已清空「$pluginName」的插件存储' : result.error;
    });
  }

  /// 打开「开发工具链」对话框（轨道 H，`docs/todo.md` §M5z-补69）。
  ///
  /// 授权 / 撤销由用户点（AI 只能改清单，不能自批）；安装与构建都同步阻塞，所以走
  /// `Isolate.run` 不卡面板，「停止」随时可用（宿主 Drop Job Object ⇒ 整棵进程树被杀）。
  ///
  /// **这不是沙箱**：文案里原样写清楚（`forbidden.md` §禁用 1），不写「隔离环境」。
  Future<void> _openPluginToolchain(String pluginId, String pluginName) async {
    var status = readToolchainStatus(pluginId);
    var note = '';
    var noteError = false;
    var busy = false;
    var steps = const <ToolchainStep>[];
    var output = '';
    await showDialog<void>(
      context: context,
      builder: (ctx) => StatefulBuilder(
        builder: (ctx, setDialogState) {
          void report(String text, bool failed) {
            setDialogState(() {
              note = text;
              noteError = failed;
            });
          }

          void refresh() {
            setDialogState(() {
              status = readToolchainStatus(pluginId);
            });
          }

          Future<void> runInstall() async {
            setDialogState(() {
              busy = true;
              note = '正在下载工具链…（按清单里的 url 下载并校验 sha256，可点「停止」）';
              noteError = false;
              steps = const [];
            });
            final result = await Isolate.run(() => installToolchain(pluginId));
            if (!ctx.mounted) {
              return;
            }
            setDialogState(() {
              busy = false;
              steps = result.steps;
              note = result.ok
                  ? '工具链已装好（${result.steps.length} 条）'
                  : result.error;
              noteError = !result.ok;
            });
            refresh();
          }

          Future<void> runBuild() async {
            setDialogState(() {
              busy = true;
              note = '正在构建…（可点「停止」）';
              noteError = false;
              output = '';
            });
            final result = await Isolate.run(() => buildToolchain(pluginId));
            if (!ctx.mounted) {
              return;
            }
            setDialogState(() {
              busy = false;
              output = result.output;
              note = result.ok
                  ? '构建成功（${(result.durationMs / 1000).toStringAsFixed(1)} 秒）'
                  : result.error;
              noteError = !result.ok;
            });
            refresh();
          }

          Future<void> runRevoke() async {
            final deleteToolchain = await _confirmRevokeToolchain(
              ctx,
              pluginName,
            );
            if (deleteToolchain == null || !ctx.mounted) {
              return;
            }
            final result = revokeToolchain(
              pluginId,
              deleteToolchain: deleteToolchain,
            );
            report(
              result.ok
                  ? '已撤销授权'
                        '${result.stopped ? '，并停掉在跑的构建' : ''}'
                        '${result.deleted ? '，已删除 .toolchain 目录' : ''}'
                  : result.error,
              !result.ok,
            );
            refresh();
          }

          /// 停止在跑的构建（宿主持有 Job Object，删槽位 = 整棵进程树被杀）。
          void stopBuild() {
            final result = stopToolchain(pluginId);
            report(
              result.stopped
                  ? '已发出停止（整棵进程树）'
                  : (result.error.isEmpty ? '没有在跑的构建' : result.error),
              !result.ok,
            );
            refresh();
          }

          final toolchains = status.toolchains;
          final build = status.build;
          return AlertDialog(
            backgroundColor: LunacColors.background,
            title: Text(
              '开发工具链：$pluginName',
              style: TextStyle(color: LunacColors.text, fontSize: 13.5),
            ),
            content: SizedBox(
              width: 640,
              child: SingleChildScrollView(
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    if (!status.ok)
                      Text(
                        status.error.isEmpty ? '读不了工具链状态' : status.error,
                        style: TextStyle(
                          color: LunacColors.danger,
                          fontSize: 11.5,
                        ),
                      )
                    else ...[
                      Text(
                        '装了工具链 = 本机可执行代码 + 构建期脚本执行 —— **这不是沙箱**，'
                        '只给你信任的插件授权。落点：<插件目录>\\.toolchain\\<profile>\\'
                        '（卸载插件时一起删掉）。',
                        style: TextStyle(
                          color: LunacColors.textMuted,
                          fontSize: 11,
                          height: 1.5,
                        ),
                      ),
                      if (!status.hasDev)
                        Padding(
                          padding: const EdgeInsets.only(top: 10),
                          child: Text(
                            '这个插件没有 dev 段：不需要工具链（轨道 L 只要 lunac-plugin.json + index.js）。',
                            style: TextStyle(
                              color: LunacColors.textMuted,
                              fontSize: 11.5,
                            ),
                          ),
                        ),
                      const SizedBox(height: 10),
                      Row(
                        children: [
                          if (!status.granted)
                            _settingsButton(
                              '授权',
                              busy
                                  ? null
                                  : () {
                                      final result = grantToolchain(pluginId);
                                      report(
                                        result.ok
                                            ? '已授权：dev 段的工具链与构建命令现在可以跑了'
                                            : result.error,
                                        !result.ok,
                                      );
                                      if (result.ok) {
                                        refresh();
                                      }
                                    },
                            ),
                          if (status.granted)
                            _settingsButton('撤销', busy ? null : runRevoke),
                          if (status.granted && toolchains.isNotEmpty)
                            _settingsButton('下载工具链', busy ? null : runInstall),
                          if (status.granted && build != null)
                            _settingsButton('构建', busy ? null : runBuild),
                          if (busy) _settingsButton('停止', stopBuild),
                        ],
                      ),
                      for (final chain in toolchains) ...[
                        const SizedBox(height: 10),
                        Container(
                          padding: const EdgeInsets.symmetric(
                            horizontal: 10,
                            vertical: 8,
                          ),
                          decoration: BoxDecoration(
                            color: LunacColors.background,
                            border: Border.all(color: LunacColors.border),
                            borderRadius: BorderRadius.circular(8),
                          ),
                          child: Column(
                            crossAxisAlignment: CrossAxisAlignment.start,
                            children: [
                              Row(
                                children: [
                                  Text(
                                    chain.label,
                                    style: TextStyle(
                                      color: LunacColors.text,
                                      fontSize: 12.5,
                                    ),
                                  ),
                                  const SizedBox(width: 6),
                                  _skillBadge(
                                    chain.kind == 'archive' ? '压缩包' : '单文件',
                                    LunacColors.textMuted,
                                  ),
                                  const SizedBox(width: 4),
                                  _skillBadge(
                                    chain.installed ? '已安装' : '未安装',
                                    chain.installed
                                        ? LunacColors.accent
                                        : LunacColors.textMuted,
                                  ),
                                ],
                              ),
                              const SizedBox(height: 2),
                              Text(
                                '落点 ${chain.dest}'
                                '${chain.binDir.isEmpty ? '' : ' · bin ${chain.binDir}'}'
                                ' · sha256 ${chain.sha256.length > 12 ? chain.sha256.substring(0, 12) : chain.sha256}…',
                                style: TextStyle(
                                  color: LunacColors.textMuted,
                                  fontSize: 10,
                                ),
                              ),
                              Text(
                                chain.url,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: TextStyle(
                                  color: LunacColors.textDim,
                                  fontSize: 10,
                                  fontFamily: 'Consolas',
                                  fontFamilyFallback: const [
                                    'Courier New',
                                    'monospace',
                                  ],
                                ),
                              ),
                            ],
                          ),
                        ),
                      ],
                      if (build != null) ...[
                        const SizedBox(height: 10),
                        Text(
                          '构建：${build.label}'
                          '${build.cwd.isEmpty ? '' : '（cwd ${build.cwd}）'}',
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                            color: LunacColors.text,
                            fontSize: 11.5,
                            fontFamily: 'Consolas',
                            fontFamilyFallback: const [
                              'Courier New',
                              'monospace',
                            ],
                          ),
                        ),
                      ],
                      if (build == null && toolchains.isEmpty) ...[
                        const SizedBox(height: 8),
                        Text(
                          '清单里只声明了工具链、没有构建命令：下载完由你自己在终端里跑。',
                          style: TextStyle(
                            color: LunacColors.textMuted,
                            fontSize: 10.5,
                          ),
                        ),
                      ],
                      if (steps.isNotEmpty) ...[
                        const SizedBox(height: 8),
                        for (final step in steps)
                          Text(
                            '${step.ok ? '✓' : '✗'} ${step.id}'
                            '${step.ok ? '（${formatByteSize(step.bytes)}）' : ''}'
                            '${step.ok ? ' → ${step.dest}' : ''}',
                            style: TextStyle(
                              color: step.ok
                                  ? LunacColors.textMuted
                                  : LunacColors.danger,
                              fontSize: 10.5,
                            ),
                          ),
                      ],
                      if (output.isNotEmpty) ...[
                        const SizedBox(height: 8),
                        Container(
                          height: 150,
                          padding: const EdgeInsets.all(8),
                          decoration: BoxDecoration(
                            color: LunacColors.surface,
                            borderRadius: BorderRadius.circular(6),
                            border: Border.all(color: LunacColors.divider),
                          ),
                          child: SingleChildScrollView(
                            child: SelectableText(
                              output,
                              style: TextStyle(
                                color: LunacColors.textDim,
                                fontSize: 10,
                                height: 1.4,
                                fontFamily: 'Consolas',
                                fontFamilyFallback: const [
                                  'Courier New',
                                  'monospace',
                                ],
                              ),
                            ),
                          ),
                        ),
                      ],
                      if (note.isNotEmpty)
                        Padding(
                          padding: const EdgeInsets.only(top: 8),
                          child: Text(
                            note,
                            style: TextStyle(
                              color: noteError
                                  ? LunacColors.danger
                                  : LunacColors.textMuted,
                              fontSize: 11,
                            ),
                          ),
                        ),
                      Padding(
                        padding: const EdgeInsets.only(top: 8),
                        child: Text(
                          '清单指纹：${status.fingerprint.length > 12 ? status.fingerprint.substring(0, 12) : status.fingerprint}'
                          ' —— 改任何一个清单元件，旧授权都会失效。',
                          style: TextStyle(
                            color: LunacColors.textMuted,
                            fontSize: 10,
                          ),
                        ),
                      ),
                    ],
                  ],
                ),
              ),
            ),
            actions: [
              TextButton(
                onPressed: () => Navigator.of(ctx).pop(),
                child: const Text('关闭'),
              ),
            ],
          );
        },
      ),
    );
  }

  /// 撤销确认：返回 `null` = 取消；`false` = 只撤销授权；`true` = 连 `.toolchain\` 一起删。
  ///
  /// 删目录是**用户的选择**（README：撤销可选同时删除），不默认删 —— 重装一次要重新下载。
  Future<bool?> _confirmRevokeToolchain(BuildContext ctx, String pluginName) {
    return showDialog<bool>(
      context: ctx,
      builder: (dialogCtx) => AlertDialog(
        backgroundColor: LunacColors.background,
        title: Text(
          '撤销「$pluginName」的开发工具链授权',
          style: TextStyle(color: LunacColors.text, fontSize: 13.5),
        ),
        content: SizedBox(
          width: 460,
          child: Text(
            '撤销后立刻停掉在跑的构建，且必须重新授权才能再下载 / 构建。\n'
            '`.toolchain\\` 里的文件要不要一起删？删掉之后重装需要重新下载。',
            style: TextStyle(
              color: LunacColors.textMuted,
              fontSize: 11.5,
              height: 1.55,
            ),
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(dialogCtx).pop(),
            child: const Text('取消'),
          ),
          TextButton(
            onPressed: () => Navigator.of(dialogCtx).pop(false),
            child: const Text('只撤销授权'),
          ),
          TextButton(
            onPressed: () => Navigator.of(dialogCtx).pop(true),
            child: Text('撤销并删除目录', style: TextStyle(color: LunacColors.danger)),
          ),
        ],
      ),
    );
  }

  /// 卸载一条已装插件（整个目录删掉）。
  Future<void> _uninstallMarket(InstalledPlugin plugin) async {
    if (_marketBusyId != null) {
      return;
    }
    setState(() {
      _marketBusyId = plugin.id;
      _marketError = false;
      _marketMessage = '正在卸载「${plugin.name}」…';
    });
    final raw = await Isolate.run(() => uninstallMarketPluginRaw(plugin.id));
    if (!mounted) {
      return;
    }
    final result = MarketActionResult.fromRaw(raw, plugin.id);
    setState(() {
      _marketBusyId = null;
      _marketError = !result.ok;
      _marketMessage = result.ok ? '已卸载「${plugin.name}」' : result.error;
    });
    if (result.ok) {
      await _loadMarket();
    }
  }

  /// 三个来源合成一张表：已装（含坏包）→ 索引里本机还没有的。
  ///
  /// 版本号只判「不同」不判大小（索引里的版本是作者写的自由文本，比大小只会比出误报）。
  List<_MarketRow> _mergeMarketRows() {
    final out = <_MarketRow>[];
    final installed = _marketInstalled?.entries ?? const <InstalledPlugin>[];
    final index = _marketIndex?.entries ?? const <MarketIndexEntry>[];
    final indexById = {for (final e in index) e.id: e};
    final seen = <String>{};
    for (final p in installed) {
      seen.add(p.id);
      final e = indexById[p.id];
      final hasNewer =
          e != null &&
          (p.version.isEmpty || e.version.isEmpty || e.version != p.version);
      out.add(
        _MarketRow(
          id: p.id,
          name: p.name,
          description: p.description,
          icon: p.icon,
          version: p.version,
          installed: true,
          valid: p.valid,
          error: p.error,
          deps: p.deps,
          permissions: p.permissions,
          hasSidecar: p.hasSidecar,
          url: hasNewer ? e.url : '',
          sha256: hasNewer ? e.sha256 : '',
          update: hasNewer,
        ),
      );
    }
    for (final e in index) {
      if (seen.contains(e.id)) {
        continue;
      }
      out.add(
        _MarketRow(
          id: e.id,
          name: e.name,
          description: e.description,
          icon: e.icon,
          version: e.version,
          installed: false,
          valid: true,
          error: '',
          deps: 0,
          permissions: const [],
          url: e.url,
          sha256: e.sha256,
          update: false,
        ),
      );
    }
    return out;
  }

  List<Widget> _buildMarketRows() {
    final rows = <Widget>[];
    final installed = _marketInstalled;
    final message = _marketMessage;

    // v1 `buildPluginsPane`：分组标题下先两行说明，列表末尾才是右对齐的「重新扫描」。
    rows.add(_hint('插件是 Lunac 专用的拓展功能模块。'));
    rows.add(
      _hint(
        '插件装在 <exe 根>\\Modules\\<id>。下载的是可执行代码，装谁 = 信任谁；'
        '清单里声明了 sidecar 的插件，可以用任意语言写自己的进程 —— 点「运行时」起它、'
        '看信任卡与事件；v2 不加载第三方 WebView（桌宠的 Live2D 除外）。',
      ),
    );
    if (message != null) {
      rows.add(
        Padding(
          padding: const EdgeInsets.only(top: 6),
          child: Text(
            message,
            style: TextStyle(
              color: _marketError ? LunacColors.danger : LunacColors.textMuted,
              fontSize: 11,
            ),
          ),
        ),
      );
    }
    final progress = _marketProgressRow();
    if (progress != null) {
      rows.add(progress);
    }
    if (installed == null) {
      rows.add(
        Padding(
          padding: EdgeInsets.only(top: 8),
          child: Text(
            '不可用（rust 桥接未加载）',
            style: TextStyle(color: LunacColors.textMuted, fontSize: 12),
          ),
        ),
      );
      rows.add(_marketRescanRow());
      return rows;
    }
    if (installed.error.isNotEmpty) {
      rows.add(
        Padding(
          padding: const EdgeInsets.only(top: 6),
          child: Text(
            installed.error,
            style: TextStyle(color: LunacColors.danger, fontSize: 11),
          ),
        ),
      );
    }
    final indexError = _marketIndex?.error ?? '';
    if (indexError.isNotEmpty) {
      rows.add(
        Padding(
          padding: const EdgeInsets.only(top: 6),
          child: Text(
            indexError,
            style: TextStyle(color: LunacColors.danger, fontSize: 11),
          ),
        ),
      );
    }

    final merged = _mergeMarketRows();
    if (merged.isEmpty && !_marketLoading) {
      rows.add(
        Padding(
          padding: EdgeInsets.only(top: 8),
          child: Text(
            '市场里还没有可安装的插件',
            style: TextStyle(color: LunacColors.textMuted, fontSize: 12),
          ),
        ),
      );
    }
    for (final row in merged) {
      rows.add(_buildMarketRowTile(row));
    }
    if (installed.dir.isNotEmpty) {
      rows.add(_hint('插件目录：${installed.dir}'));
    }
    rows.add(_marketRescanRow());
    return rows;
  }

  /// 安装进度条（v1「本次安装」那一行的等价物）：百分比 / 已下字节 / 暂停 / 取消。
  ///
  /// 没有活动下载时若有断点，也如实说明「点下载会接着下」—— 不假装在跑。
  Widget? _marketProgressRow() {
    final st = _marketState;
    if (!st.active) {
      if (_marketPending.isEmpty) {
        return null;
      }
      final names = _marketPending.map(_pendingName).join('、');
      return Padding(
        padding: const EdgeInsets.only(top: 6),
        child: Text(
          '有未完成的下载：$names —— 点对应行的「继续下载」接着下'
          '（关面板 / 重启应用都不丢；服务端不支持 Range 时退化成从头下）',
          style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
        ),
      );
    }
    final title = st.phase == 'extract'
        ? '解压'
        : switch (st.kind) {
            'deps' when st.index > 0 => '依赖 ${st.index}/${st.count}',
            'deps' => '依赖',
            _ => '插件包',
          };
    final who = st.id.isNotEmpty ? st.id : urlTail(st.url);
    final size = st.total > 0
        ? '${formatByteSize(st.downloaded)} / ${formatByteSize(st.total)}'
        : formatByteSize(st.downloaded);
    final pct = percentText(st.fraction);
    final label = st.paused
        ? '$title · $who · 已暂停 · $size'
        : '$title · $who · ${pct.isEmpty ? '下载中' : pct} · $size';
    return Padding(
      padding: const EdgeInsets.only(top: 8),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
        decoration: BoxDecoration(
          color: LunacColors.background,
          border: Border.all(color: LunacColors.border),
          borderRadius: BorderRadius.circular(8),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Expanded(
                  child: Text(
                    label,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: st.paused
                          ? LunacColors.textMuted
                          : LunacColors.text,
                      fontSize: 12,
                    ),
                  ),
                ),
                _settingsButton(st.paused ? '继续' : '暂停', _toggleMarketPause),
                _settingsButton('取消', _cancelMarketInstall),
              ],
            ),
            const SizedBox(height: 6),
            ClipRRect(
              borderRadius: BorderRadius.circular(2),
              child: LinearProgressIndicator(
                value: st.fraction,
                minHeight: 4,
                backgroundColor: LunacColors.border,
                valueColor: AlwaysStoppedAnimation<Color>(LunacColors.accent),
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildMarketRowTile(_MarketRow row) {
    final busy = _marketBusyId == row.id;
    final pending = _pendingForRow(row);
    final pendingPct = pending?.fraction;
    final downloadLabel = pending != null
        ? (pendingPct == null
              ? '继续下载'
              : '继续下载 (${(pendingPct * 100).floor()}%)')
        : (row.update ? '更新' : (row.installed ? '重装' : '下载'));
    final meta = <String>[
      if (row.version.isNotEmpty) 'v${row.version}',
      if (!row.installed) '可安装' else if (!row.valid) '已损坏' else '已安装',
      if (row.deps > 0) '依赖 ${row.deps} 个',
      if (row.permissions.isNotEmpty) '能力 ${row.permissions.join(' / ')}',
    ].join(' · ');

    return Padding(
      padding: const EdgeInsets.only(top: 8),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
        decoration: BoxDecoration(
          color: LunacColors.background,
          border: Border.all(
            color: row.installed && !row.valid
                ? LunacColors.danger
                : LunacColors.border,
          ),
          borderRadius: BorderRadius.circular(8),
        ),
        child: Row(
          children: [
            SizedBox(
              width: 22,
              child: Text(
                row.icon.isEmpty ? '🧩' : row.icon,
                textAlign: TextAlign.center,
                style: const TextStyle(fontSize: 15),
              ),
            ),
            const SizedBox(width: 8),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    row.name,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(color: LunacColors.text, fontSize: 13),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    row.description.isEmpty ? meta : '${row.description}｜$meta',
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: LunacColors.textMuted,
                      fontSize: 11,
                    ),
                  ),
                  if (row.error.isNotEmpty)
                    Padding(
                      padding: const EdgeInsets.only(top: 2),
                      child: Text(
                        row.error,
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          color: LunacColors.danger,
                          fontSize: 11,
                        ),
                      ),
                    ),
                ],
              ),
            ),
            if (busy)
              Padding(
                padding: EdgeInsets.only(left: 8),
                child: Text(
                  '处理中…',
                  style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
                ),
              ),
            if (row.url.isNotEmpty)
              _settingsButton(
                downloadLabel,
                _marketBusyId == null
                    ? () => _installMarket(
                        MarketIndexEntry(
                          id: row.id,
                          name: row.name,
                          description: row.description,
                          version: row.version,
                          url: row.url,
                          sha256: row.sha256,
                          icon: row.icon,
                          homepage: '',
                          keywords: const [],
                        ),
                      )
                    : null,
              ),
            if (row.installed && row.valid && row.deps > 0)
              _settingsButton(
                '补依赖',
                _marketBusyId == null
                    ? () => _installMarketDeps(row.id, row.name)
                    : null,
              ),
            if (row.installed && row.valid && row.hasSidecar)
              _settingsButton('运行时', () => _openPluginRuntime(row.id)),
            if (row.installed && row.valid)
              _settingsButton('存储', () => _openPluginStorage(row.id, row.name)),
            if (row.installed && row.valid)
              // 轨道 H 的入口：只有装了 & 清单有效的插件才有工具链可谈（没有 dev 段也进得去，
              // 对话框会如实说「不需要工具链」）。
              _settingsButton(
                '工具链',
                () => _openPluginToolchain(row.id, row.name),
              ),
            if (row.installed)
              _settingsButton(
                '卸载',
                _marketBusyId == null
                    ? () => _uninstallMarket(
                        InstalledPlugin(
                          id: row.id,
                          name: row.name,
                          description: row.description,
                          version: row.version,
                          icon: row.icon,
                          homepage: '',
                          dir: '',
                          entryPath: '',
                          valid: row.valid,
                          error: row.error,
                          deps: row.deps,
                          permissions: row.permissions,
                        ),
                      )
                    : null,
              ),
          ],
        ),
      ),
    );
  }

  /// 市场底部的「重新扫描」（v1 是列表末尾右对齐的一行）。
  Widget _marketRescanRow() {
    return Row(
      children: [
        const Spacer(),
        _settingsButton(
          _marketLoading ? '拉取中…' : '重新扫描',
          _marketLoading ? null : _loadMarket,
        ),
      ],
    );
  }

  /// 切「自动更新」：写 `config/update.json` 的两个字段（同真同假），失败如实显示。
  void _toggleUpdateAuto(bool on) {
    final error = setUpdateAuto(on);
    setState(() {
      if (error == null) {
        _updateAuto = on;
        _updateAutoMessage = on ? '已开启：启动时自动检查并安装新版本' : '已关闭：不再自动检查更新';
        _updateAutoError = false;
      } else {
        _updateAutoMessage = error;
        _updateAutoError = true;
      }
    });
  }

  /// 打开 `config/pricing.json`（缺文件时后端先落一份预置骨架再交给系统默认程序）。
  void _openPricing() {
    final path = ensurePricingFile();
    if (path == null) {
      setState(() {
        _costError = true;
        _costMessage = '定价表不可用（未加载 rust 桥接）';
      });
      return;
    }
    openPath(path);
    setState(() {
      _pricing = readPricingState();
      _costError = false;
      _costMessage = '已打开 $path';
    });
  }

  /// 「更新价格」：把抓取任务交给常驻 agent（v1 `__lunac_agent_task` 同口径）。
  ///
  /// 只读档位下 agent 连 Write 都过不去 —— 与其发一条注定失败的提示词，不如在这里说清楚。
  /// 结果只写进候选文件，用户回到这里点「确认写入」才覆盖正式表。
  void _updatePrices() {
    final prefs = readChatSettings();
    if (prefs.profile == 'safe') {
      setState(() {
        _costError = true;
        _costMessage = '当前安全档位是「只读」：agent 不能写文件，「更新价格」只在「项目」或「完整」档位下可用。';
      });
      return;
    }
    final state = readPricingState();
    final wanted = <String>{
      for (final day in readUsageRange())
        for (final model in day.models)
          if (model.model.isNotEmpty) model.model,
    };
    final current = _aiModel.text.trim();
    if (current.isNotEmpty) {
      wanted.add(current);
    }
    if (wanted.isEmpty) {
      setState(() {
        _costError = false;
        _costMessage = '近 $costRangeDays 天没有用量记录。';
      });
      return;
    }
    final prompt = _costFetchPrompt(wanted, state.pendingPath);
    final reason = startAgent();
    if (reason != null ||
        !sendAgentLine(jsonEncode(userMessagePayload(prompt)))) {
      setState(() {
        _costError = true;
        _costMessage = '操作失败：${reason ?? 'agent 未就绪'}';
      });
      return;
    }
    setState(() {
      _pricing = state;
      _costError = false;
      _costMessage = '已把抓取任务交给 agent：抓完回到这里点「确认写入」。';
    });
  }

  /// 确认候选价格（先校验后覆盖；不合法一个字都不写）。
  void _confirmPricing() {
    final error = commitPricingPending(localDateKey(DateTime.now()));
    setState(() {
      _pricing = readPricingState();
      _costError = error != null;
      _costMessage = error ?? '已写入 pricing.json';
    });
  }

  void _discardPricing() {
    final error = discardPricingPending();
    setState(() {
      _pricing = readPricingState();
      _costError = error != null;
      _costMessage = error ?? '已放弃候选价格';
    });
  }

  /// v1 `settings.cost_fetch_prompt` 的中文原文（只替换模型列表与候选文件路径）。
  static String _costFetchPrompt(Set<String> models, String path) {
    final list = models.map((model) => '- $model').join('\n');
    return '帮我更新本地模型价格表。单价单位是「元 / 百万 token」，四类分别计价：'
        'input（未命中缓存的输入）/ cache_read（命中缓存）/ cache_write（缓存写入）/ output（输出）。\n\n'
        '请先用 WebSearch / WebFetch 查这些模型在**官方定价页**的最新单价：\n'
        '$list\n\n'
        '再用 Write 把结果写到这个文件：\n$path\n\n'
        '格式（可以先 Read 现有的 config\\pricing.json 当样板）：\n'
        '```json\n'
        '{\n'
        '  "models": {\n'
        '    "<模型名>": { "input": 0, "cache_read": 0, "cache_write": 0, "output": 0, '
        '"source_url": "<官方定价页 URL>", "updated_at": "YYYY-MM-DD" }\n'
        '  }\n'
        '}\n'
        '```\n\n'
        '要求：① **不要改 config\\pricing.json** —— 确认由我在设置面板里做；'
        '② 沿用旧值的模型保持原来的 source_url / updated_at；③ 查不到的如实说明，不要编数字。\n\n'
        '若官方定价页有**峰谷 / 分时价**，照现有 config\\pricing.json 里 `time_windows` 的写法补上：'
        '基础四类价填**谷时（缺省）价**，`time_windows` 里填各时段生效的价（含 days / from / to）。';
  }

  /// 检查更新（手动触发）。失败如实显示原因，不假装「已是最新」。
  Future<void> _checkUpdate() async {
    if (_checkingUpdate || _installing) {
      return;
    }
    setState(() {
      _checkingUpdate = true;
      _update = null;
    });
    final result = await checkForUpdate();
    if (!mounted) {
      return;
    }
    setState(() {
      _checkingUpdate = false;
      _update = result;
    });
  }

  /// 第一次点「下载并安装」只是上膛：按钮变成「再点一次确认」，3 秒内不点第二次就还原。
  void _armInstall() {
    _confirmInstallTimer?.cancel();
    setState(() => _confirmInstall = true);
    _confirmInstallTimer = Timer(const Duration(seconds: 3), () {
      if (!mounted) {
        return;
      }
      setState(() => _confirmInstall = false);
    });
  }

  /// 下载 → sha256 校验 → 静默安装 → 退出交棒（安装器覆盖 exe 后重新拉起应用）。
  Future<void> _installUpdate(UpdateStatus status) async {
    if (_installing) {
      return;
    }
    final dir = dataDirectory() ?? _dataDir;
    if (dir == null || dir.isEmpty) {
      setState(() {
        _installError = true;
        _installMessage = '定位不了安装目录';
      });
      return;
    }
    _confirmInstallTimer?.cancel();
    setState(() {
      _installing = true;
      _confirmInstall = false;
      _installProgress = null;
      _installError = false;
      _installMessage = '下载中…';
    });

    final result = await downloadInstaller(
      status,
      installDir: dir,
      onProgress: (downloaded, total) {
        if (!mounted) {
          return;
        }
        setState(() {
          if (total > 0) {
            final percent = downloaded * 100 / total;
            _installProgress = percent;
            _installMessage = '下载中 ${percent.toStringAsFixed(0)}%';
          } else {
            _installProgress = null;
            _installMessage = '下载中…';
          }
        });
      },
    );
    if (!mounted) {
      return;
    }

    final installerPath = result.installerPath;
    if (installerPath == null) {
      setState(() {
        _installing = false;
        _installProgress = null;
        _installError = true;
        _installMessage = result.error ?? '下载安装包失败 —— 请稍后重试';
      });
      return;
    }

    setState(() {
      _installProgress = 100;
      _installMessage = '正在启动安装程序…';
    });
    final reason = await launchInstaller(installerPath, installDir: dir);
    if (!mounted) {
      return;
    }
    if (reason != null) {
      setState(() {
        _installing = false;
        _installProgress = null;
        _installError = true;
        _installMessage = reason;
      });
      return;
    }
    // 交棒：安装器会覆盖 exe 并重新拉起；测试环境无平台通道时 quitApp 自身已兜底。
    await quitApp();
    if (!mounted) {
      return;
    }
    setState(() {
      _installing = false;
      _installProgress = null;
    });
  }

  // ── AI 模型 ────────────────────────────────────────────────────

  /// 切供应商：端点按 v1 规则**直接覆盖**（自定义 / 未知供应商清空），模型只在
  /// 「当前模型属于别的供应商」时才回落预设默认值（v1 `reloadModelSelector` 口径）。
  void _selectAiProvider(String key) {
    if (key == _aiProvider) {
      return;
    }
    final preset = providerPreset(key);
    setState(() {
      _aiProvider = key;
      _aiMessage = null;
      _aiUrl.text = preset.url;
      _aiModel.text = modelAfterProviderSwitch(
        currentModel: _aiModel.text,
        provider: key,
      );
    });
  }

  void _saveAi() {
    final url = _aiUrl.text.trim();
    final model = _aiModel.text.trim();
    final key = _aiKey.text.trim();
    if (url.isEmpty || model.isEmpty || key.isEmpty) {
      setState(() {
        _aiError = true;
        _aiMessage = '接口地址 / 模型 / API 密钥都不能为空';
      });
      return;
    }
    // vision 与 v1 的精简口径一致：恒开，模型自己判断收不收图片。
    final reason = saveAiConfig({
      'provider': _aiProvider,
      'url': url,
      'key': key,
      'model': model,
      'vision': true,
    });
    setState(() {
      _aiError = reason != null;
      _aiMessage = reason ?? '已保存（点对话面板的「新对话」后生效）';
    });
  }

  /// v1「命令添加」：粘贴一段 curl → 解析出供应商 / 地址 / 密钥 / 模型并填进输入框。
  Future<void> _importCurl() async {
    final controller = TextEditingController();
    String? error;
    final parsed = await showDialog<CurlParsed>(
      context: context,
      builder: (dialogContext) => StatefulBuilder(
        builder: (context, setDialogState) => AlertDialog(
          backgroundColor: LunacColors.surface,
          title: Text(
            '粘贴 curl 命令',
            style: TextStyle(
              color: LunacColors.text,
              fontSize: 13,
              fontWeight: FontWeight.w600,
            ),
          ),
          content: SizedBox(
            width: 520,
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                TextField(
                  controller: controller,
                  minLines: 4,
                  maxLines: 7,
                  autofocus: true,
                  style: TextStyle(
                    color: LunacColors.textDim,
                    fontSize: 11,
                    fontFamily: 'Consolas',
                  ),
                  cursorColor: LunacColors.accent,
                  decoration: InputDecoration(
                    hintText: '粘贴完整的 curl 命令…',
                    hintStyle: TextStyle(
                      color: LunacColors.textMuted,
                      fontSize: 11,
                    ),
                    filled: true,
                    fillColor: LunacColors.background,
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
                      borderSide: BorderSide(
                        color: LunacColors.accent.withValues(alpha: 0.6),
                      ),
                    ),
                  ),
                ),
                if (error != null)
                  Padding(
                    padding: const EdgeInsets.only(top: 6),
                    child: Text(
                      error!,
                      style: TextStyle(color: LunacColors.danger, fontSize: 11),
                    ),
                  ),
              ],
            ),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.of(dialogContext).pop(),
              child: const Text('取消'),
            ),
            TextButton(
              onPressed: () {
                final result = parseCurlConfig(controller.text);
                if (!result.ok) {
                  setDialogState(() => error = result.error);
                  return;
                }
                Navigator.of(dialogContext).pop(result.parsed);
              },
              child: const Text('解析并添加'),
            ),
          ],
        ),
      ),
    );
    controller.dispose();
    if (parsed == null || !mounted) {
      return;
    }
    _applyCurl(parsed);
  }

  void _applyCurl(CurlParsed parsed) {
    final notes = <String>[];
    if (parsed.path.isNotEmpty && parsed.path != '/') {
      notes.add('路径 ${parsed.path} 未采用（接口地址只取 origin）');
    }
    if (parsed.envKey.isNotEmpty) {
      notes.add('curl 用的是环境变量 ${parsed.envKey}，请把真实密钥填进密钥框');
    } else if (!parsed.hasKey) {
      notes.add('没找到 Authorization 头：请自己填 API 密钥');
    }
    setState(() {
      if (parsed.provider.isNotEmpty) {
        _aiProvider = parsed.provider;
      }
      _aiUrl.text = parsed.baseUrl;
      if (parsed.model.isNotEmpty) {
        _aiModel.text = parsed.model;
      }
      if (parsed.apiKey.isNotEmpty) {
        _aiKey.text = parsed.apiKey;
      }
      _aiError = false;
      final head = parsed.model.isEmpty
          ? '已填入供应商 / 地址 · 未解析到模型名，请选好模型再保存'
          : '已填入：$_aiProvider';
      _aiMessage = notes.isEmpty ? head : '$head · ${notes.join('；')}';
    });
  }

  /// 测试连通（v1 `settings-test-ai-btn` 的等价物）：用**眼前这三项**打一次
  /// `POST {接口地址}/chat/completions`，`max_tokens = 1`。
  /// 只判断「能不能通」，不读正文 —— 失败文案写清是哪一步（密钥 / 路径 / 网络）。
  Future<void> _testAi() async {
    if (_aiTesting) {
      return;
    }
    final url = _aiUrl.text.trim().replaceAll(RegExp(r'/+$'), '');
    final model = _aiModel.text.trim();
    final key = _aiKey.text.trim();
    if (url.isEmpty || model.isEmpty || key.isEmpty) {
      setState(() {
        _aiError = true;
        _aiMessage = '接口地址 / 模型 / API 密钥都不能为空';
      });
      return;
    }
    setState(() {
      _aiTesting = true;
      _aiMessage = '测试中...';
    });
    final client = HttpClient()
      ..connectionTimeout = const Duration(seconds: 10);
    try {
      final request = await client
          .postUrl(Uri.parse('$url/chat/completions'))
          .timeout(const Duration(seconds: 15));
      request.headers.contentType = ContentType.json;
      request.headers.set('Authorization', 'Bearer $key');
      request.write(
        jsonEncode({
          'model': model,
          'messages': [
            {'role': 'user', 'content': 'ping'},
          ],
          'max_tokens': 1,
        }),
      );
      final response = await request.close().timeout(
        const Duration(seconds: 20),
      );
      final code = response.statusCode;
      // 把正文丢掉，否则连接不会被归还（长响应会把面板吊住）。
      await response.drain<void>();
      if (!mounted) {
        return;
      }
      setState(() {
        _aiTesting = false;
        _aiError = code < 200 || code >= 300;
        _aiMessage = _aiError ? _testAiErrorText(code) : '连接正常';
      });
    } catch (error) {
      if (!mounted) {
        return;
      }
      setState(() {
        _aiTesting = false;
        _aiError = true;
        _aiMessage = '连接失败：$error';
      });
    } finally {
      client.close(force: true);
    }
  }

  /// HTTP 状态码 → 用户能据此动手的一句话（不贴原始 body）。
  static String _testAiErrorText(int code) {
    switch (code) {
      case 401:
      case 403:
        return 'API 密钥无效（HTTP $code）';
      case 404:
        return '接口地址或模型不存在（HTTP 404）';
      case 429:
        return '请求过于频繁（HTTP 429），稍后再试';
      default:
        return '请求失败（HTTP $code）';
    }
  }

  KeyEventResult _onKey(FocusNode node, KeyEvent event) {
    // 录制期间接管全部按键：Esc 取消，其余按「修饰键 + 主键」组成组合串。
    if (_recordingHotkey) {
      if (event is KeyDownEvent) {
        if (event.logicalKey == LogicalKeyboardKey.escape) {
          setState(() => _recordingHotkey = false);
          return KeyEventResult.handled;
        }
        final combo = _comboFromEvent(event);
        if (combo != null) {
          _applyHotkey(combo);
        }
      }
      return KeyEventResult.handled;
    }
    if (event is KeyDownEvent &&
        event.logicalKey == LogicalKeyboardKey.escape) {
      widget.onClose();
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  void _toggleRecording() {
    setState(() {
      _recordingHotkey = !_recordingHotkey;
      _hotkeyMessage = null;
    });
    if (_recordingHotkey) {
      // 录制期间必须由面板收键（点按钮后焦点可能在按钮上，先拉回面板）。
      _panelFocus.requestFocus();
    }
  }

  /// 一次按键 → 组合串（`Ctrl+Alt+Space`）；没有修饰键 / 不认识的键返回 null。
  String? _comboFromEvent(KeyEvent event) {
    final keyboard = HardwareKeyboard.instance;
    final parts = <String>[
      if (keyboard.isControlPressed) 'Ctrl',
      if (keyboard.isAltPressed) 'Alt',
      if (keyboard.isShiftPressed) 'Shift',
      if (keyboard.isMetaPressed) 'Win',
    ];
    if (parts.isEmpty) {
      return null;
    }
    final name = _keyComboName(event.logicalKey);
    if (name == null) {
      return null;
    }
    return '${parts.join('+')}+$name';
  }

  /// 逻辑键 → Rust `key_name_to_vk` 认的键名（与 v1 的热键录制同一套口径）。
  static String? _keyComboName(LogicalKeyboardKey key) {
    switch (key) {
      case LogicalKeyboardKey.space:
        return 'Space';
      case LogicalKeyboardKey.enter:
      case LogicalKeyboardKey.numpadEnter:
        return 'Enter';
      case LogicalKeyboardKey.tab:
        return 'Tab';
      case LogicalKeyboardKey.backspace:
        return 'Backspace';
      case LogicalKeyboardKey.delete:
        return 'Delete';
      case LogicalKeyboardKey.insert:
        return 'Insert';
      case LogicalKeyboardKey.home:
        return 'Home';
      case LogicalKeyboardKey.end:
        return 'End';
      case LogicalKeyboardKey.pageUp:
        return 'PageUp';
      case LogicalKeyboardKey.pageDown:
        return 'PageDown';
      case LogicalKeyboardKey.arrowUp:
        return 'Up';
      case LogicalKeyboardKey.arrowDown:
        return 'Down';
      case LogicalKeyboardKey.arrowLeft:
        return 'Left';
      case LogicalKeyboardKey.arrowRight:
        return 'Right';
      case LogicalKeyboardKey.period:
        return '.';
      case LogicalKeyboardKey.comma:
        return ',';
      case LogicalKeyboardKey.slash:
        return '/';
      case LogicalKeyboardKey.backslash:
        return r'\';
      case LogicalKeyboardKey.minus:
        return '-';
      case LogicalKeyboardKey.equal:
        return '=';
      case LogicalKeyboardKey.bracketLeft:
        return '[';
      case LogicalKeyboardKey.bracketRight:
        return ']';
      case LogicalKeyboardKey.semicolon:
        return ';';
      case LogicalKeyboardKey.quote:
        return "'";
      case LogicalKeyboardKey.backquote:
        return '`';
    }
    final label = key.keyLabel.toUpperCase();
    if (RegExp(r'^[A-Z0-9]$').hasMatch(label)) {
      return label;
    }
    if (RegExp(r'^F([1-9]|1[0-2])$').hasMatch(label)) {
      return label;
    }
    return null;
  }

  void _applyHotkey(String combo) {
    setState(() => _recordingHotkey = false);
    final reason = setHotkeyCombo(combo);
    setState(() {
      _hotkeyError = reason != null;
      if (reason == null) {
        _hotkey = combo;
        _hotkeyMessage = '已改为 $combo（立即生效，已写入 config/hotkey.json）';
      } else {
        _hotkeyMessage = reason;
      }
    });
  }

  /// 拨开机自启开关。
  ///
  /// Rust 侧非管理员时会**弹一次 UAC 并阻塞等待**，所以放到独立 isolate 里跑，
  /// 避免界面卡死；结束后重新读一次状态（以实际生效结果为准，不信乐观更新）。
  Future<void> _toggleAutoStart(bool enable) async {
    if (_autoStartBusy) {
      return;
    }
    setState(() {
      _autoStartBusy = true;
      _autoStartMessage = null;
    });
    final reason = await Isolate.run(() => setAutoStart(enable));
    if (!mounted) {
      return;
    }
    setState(() {
      _autoStartBusy = false;
      _autoStartError = reason != null;
      _autoStartMessage =
          reason ?? (enable ? '已开启：登录 Windows 后自动启动' : '已关闭开机自启');
      _autoStart = readAutoStart() ?? _autoStart;
    });
  }

  @override
  Widget build(BuildContext context) {
    // 面板自己接管键盘：Esc 无论焦点在哪个子控件上都关闭面板。
    //
    // 布局对齐 v1：左侧分类抽屉，右侧只画当前分类（打开设置时搜索栏整个消失）。
    return Focus(
      focusNode: _panelFocus,
      autofocus: true,
      onKeyEvent: _onKey,
      child: Container(
        decoration: BoxDecoration(
          color: LunacColors.surface,
          border: Border.all(color: LunacColors.border),
          borderRadius: BorderRadius.circular(12),
        ),
        clipBehavior: Clip.antiAlias,
        child: Column(
          children: [
            _buildHeader(),
            Divider(height: 1, color: LunacColors.border),
            Expanded(
              child: LayoutBuilder(
                builder: (context, constraints) {
                  // v1：侧栏宽 30%（min 100px）；这里限到 180 免得宽窗时把内容挤窄。
                  final sidebarWidth = (constraints.maxWidth * 0.3)
                      .clamp(100.0, 180.0)
                      .toDouble();
                  return Row(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      _buildSidebar(sidebarWidth),
                      VerticalDivider(width: 1, color: LunacColors.border),
                      Expanded(child: _buildPane()),
                    ],
                  );
                },
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildHeader() {
    // 2026-11-20：设置面板打开后搜索栏整个消失，标题栏必须自己接住窗口拖动；
    // `onPanStart` 只在移动超过手势阈值后触发，所以左侧返回键 / 面板内控件照常可点。
    return GestureDetector(
      behavior: HitTestBehavior.opaque,
      onPanStart: (_) {
        final drag = widget.onDrag;
        if (drag != null) {
          unawaited(drag());
        } else {
          unawaited(startWindowDrag());
        }
      },
      child: Padding(
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
              '设置',
              style: TextStyle(
                color: LunacColors.text,
                fontSize: 14,
                fontWeight: FontWeight.w600,
              ),
            ),
          ],
        ),
      ),
    );
  }

  /// 左侧分类抽屉（v1 `settings-sidebar`）：宽 30%，选中时左侧 3px 强调条。
  Widget _buildSidebar(double width) {
    return SizedBox(
      width: width,
      child: Container(
        color: LunacColors.background,
        child: ListView(
          padding: const EdgeInsets.symmetric(vertical: 6),
          children: [
            for (final item in _settingsCategories)
              _buildSidebarItem(item.id, item.label),
          ],
        ),
      ),
    );
  }

  Widget _buildSidebarItem(String id, String label) {
    final selected = id == _category;
    return Material(
      color: selected ? LunacColors.surfaceHover : Colors.transparent,
      child: InkWell(
        onTap: () => _selectCategory(id),
        child: Container(
          height: 36,
          decoration: BoxDecoration(
            border: Border(
              left: BorderSide(
                width: 3,
                color: selected ? LunacColors.accent : Colors.transparent,
              ),
            ),
          ),
          padding: const EdgeInsets.only(left: 11, right: 8),
          alignment: Alignment.centerLeft,
          child: Text(
            label,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(
              color: selected ? LunacColors.text : LunacColors.textDim,
              fontSize: 12.5,
              fontWeight: selected ? FontWeight.w600 : FontWeight.w400,
            ),
          ),
        ),
      ),
    );
  }

  void _selectCategory(String id) {
    if (id == _category) {
      return;
    }
    setState(() => _category = id);
    if (id == 'plugins' && !_marketLoaded) {
      unawaited(_loadMarket());
    }
    // 技能列表只读一次目录：第一次进 AI 页时加载（之后点「刷新」）。
    if (id == 'ai' && !_skillsLoaded) {
      _refreshSkills();
    }
    // 离开插件页就别再轮询安装进度（定时器只在这一页活着）。
    if (id != 'plugins') {
      _marketPoll?.cancel();
      _marketPoll = null;
    }
  }

  /// 右侧内容区：只渲染当前分类（v1 `settings-pane` 同口径）。
  Widget _buildPane() {
    // 分区标题逐字取 v1（`settings-pane-title`）：常规 / 风格 / AI / 网页搜索 / 插件市场。
    final children = switch (_category) {
      'appearance' => [
        _paneTitle('风格'),
        // 三段与 v1 当前形态一致：背景 → 主题颜色 → 主题包。
        _section('背景', _buildBackgroundRows()),
        _section('主题颜色', _buildColorRows()),
        _section('主题包', _buildThemeRows()),
      ],
      'ai' => [
        _paneTitle('AI'),
        _section('AI 模型', _buildAiRows()),
        _section('技能', _buildSkillRows()),
        // v1 `#sp-ai` 的第三组：远端 MCP 服务器（只给「打开配置文件」，不做图形编辑器）。
        _section('工具 (MCP)', _buildMcpRows()),
        _section('用量与成本', _buildCostRows()),
      ],
      'search' => [
        _paneTitle('网页搜索'),
        _section(null, [_buildEngineRow()]),
        _section('文件索引', _buildIndexRows()),
      ],
      'plugins' => [_paneTitle('插件'), _section('插件市场', _buildMarketRows())],
      _ => [
        _paneTitle('常规'),
        _section(null, _buildGeneralRows()),
        _section(null, _buildUpdateRows()),
        _section('定时任务（实验）', _buildCronRows()),
      ],
    };
    // 每次换分类都从顶部开始（滚动位置不跨分类保留）。
    return SingleChildScrollView(
      key: ValueKey('settings-pane-$_category'),
      padding: const EdgeInsets.fromLTRB(14, 0, 14, 16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: children,
      ),
    );
  }

  /// 分区标题（v1 `.settings-pane-title`）：13px 半粗 + 底部 1px 细线。
  Widget _paneTitle(String label) {
    return Padding(
      padding: const EdgeInsets.only(top: 8, bottom: 10),
      child: Container(
        padding: const EdgeInsets.only(bottom: 6),
        decoration: BoxDecoration(
          border: Border(bottom: BorderSide(color: LunacColors.border)),
        ),
        child: Text(
          label,
          style: TextStyle(
            color: LunacColors.text,
            fontSize: 13,
            fontWeight: FontWeight.w600,
          ),
        ),
      ),
    );
  }

  /// 分组：v1 `.settings-group-title`（主题色浅底 + 左侧 3px 色条），下面直接接扁平行。
  /// [title] 为 null 时只画内容（v1 的「常规」「网页搜索」就是没有分组标题的扁平行）。
  Widget _section(String? title, List<Widget> children) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (title != null)
          Container(
            margin: const EdgeInsets.only(top: 18, bottom: 8),
            padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
            decoration: BoxDecoration(
              color: LunacColors.accent.withValues(alpha: 0.14),
              border: Border(
                left: BorderSide(width: 3, color: LunacColors.accent),
              ),
              borderRadius: BorderRadius.circular(4),
            ),
            child: Text(
              title,
              style: TextStyle(
                color: LunacColors.text,
                fontSize: 13.5,
                fontWeight: FontWeight.w600,
              ),
            ),
          ),
        ...children.map(
          (child) => Padding(
            padding: const EdgeInsets.symmetric(vertical: 4),
            child: child,
          ),
        ),
      ],
    );
  }

  Widget _label(String text) =>
      Text(text, style: TextStyle(color: LunacColors.textDim, fontSize: 12.5));

  Widget _hint(String text) => Padding(
    padding: const EdgeInsets.only(top: 4),
    child: Text(
      text,
      style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
    ),
  );

  /// v1 `.settings-btn` 的等价物（细描边 + 极浅底的小按钮）。
  Widget _settingsButton(String label, VoidCallback? onPressed) =>
      _SettingsButton(label: label, onPressed: onPressed);

  /// 「技能」分组（v1 `#sp-ai` 的技能组）：`<exe 根>/skills/<key>/SKILL.md`。
  ///
  /// 列表只读目录；新建 / 编辑 / 删除都走 Rust 的 key 白名单与大小上限。
  /// 改完必须重启 agent —— `LUNAC_SKILLS_DIR` 只在进程启动时读一次。
  List<Widget> _buildSkillRows() {
    final rows = <Widget>[
      Row(
        children: [
          _label('技能目录：<exe 根>\\skills'),
          const Spacer(),
          _settingsButton('刷新', _refreshSkills),
          const SizedBox(width: 6),
          _settingsButton('从 URL 安装', _installSkillFromUrl),
          const SizedBox(width: 6),
          _settingsButton('新建技能', _createSkill),
        ],
      ),
      // 审计留痕的唯一出口就是白箱账本（不新开列表）：指一下路，避免用户以为没记账。
      _hint(
        '安装 / 保存 / 删除都会写一条审计行（角色 user、工具 Skills.Install / Save / Delete）：'
        '在「更多设置 → 安全 → 最近决策」里能看到。',
      ),
      const SizedBox(height: 6),
      _label('精选（随包、按哈希核对）'),
    ];
    // 「精选」索引（§待办 3-28.9 ①）：清单随包分发，每条的 sha256 由打包脚本钉住；
    // 这里显示的是「本地已装版本与精选版本是否一致」，安装走同一条哈希 / 扫描 / 权限闸。
    if (_curated.error.isNotEmpty) {
      rows.add(_hint('读取精选清单失败：${_curated.error}'));
    } else if (!_curatedLoaded) {
      rows.add(_hint('正在读取精选清单…'));
    } else if (_curated.entries.isEmpty) {
      rows.add(_hint('精选清单里还没有可验证的条目（缺 sha256 的条目会被忽略）。'));
    } else {
      for (final entry in _curated.entries) {
        rows.add(
          Row(
            children: [
              SizedBox(
                width: 130,
                child: Text(
                  entry.key,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(color: LunacColors.text, fontSize: 12),
                ),
              ),
              Expanded(
                child: Text(
                  '${entry.statusLabel}'
                  '${entry.sha256.isEmpty ? '' : ' · ${entry.hashShort}'}',
                  style: TextStyle(
                    color: entry.installed
                        ? LunacColors.textMuted
                        : LunacColors.textDim,
                    fontSize: 11,
                  ),
                ),
              ),
              if (_curatedBusyKey == entry.key)
                Text(
                  '…',
                  style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
                )
              else
                _settingsButton(
                  entry.actionLabel,
                  entry.installed
                      ? null
                      : () => unawaited(_installCurated(entry)),
                ),
            ],
          ),
        );
      }
      rows.add(
        _hint(
          '精选来自随包镜像，不联网；${_curated.updated.isEmpty ? '' : '清单日期 ${_curated.updated}，'}'
          '安装后会写 Skills.Install 审计行。',
        ),
      );
    }
    // §待办 3-28.11 的预算告警落点：同一份 `skills_listing`（agent 的 `system/init`），
    // 数字不在前端重算 —— 这里只是把对话流里那条一次性旁注搬成常驻提示。
    if (chatSession.skillOmitted > 0) {
      rows.add(
        _hint(
          '清单超出预算：共 ${chatSession.skillTotal} 个技能，只有 '
          '${chatSession.skillListed} 个进了模型提示词（其余 ${chatSession.skillOmitted} '
          '个模型看不到）。缩短技能 frontmatter 的 description 即可全部列出。',
        ),
      );
    }
    if (_skills.error.isNotEmpty) {
      rows.add(_hint(_skills.error));
    }
    if (!_skillsLoaded) {
      rows.add(_hint('正在读取…'));
      return rows;
    }
    if (_skills.entries.isEmpty) {
      rows.add(_hint('还没有技能。一个技能 = 一个目录 + SKILL.md；模型只在需要时按需读取正文。'));
    }
    for (final skill in _skills.entries) {
      rows.add(_buildSkillRow(skill));
    }
    // 技能级记忆（§待办 3-28.10 ①）：只列真有名目的技能；预览 / 清空都是用户显式动作。
    rows.add(const SizedBox(height: 6));
    rows.add(_label('技能记忆（${_skillMemories.items.length} 个）'));
    if (_skillMemories.error.isNotEmpty) {
      rows.add(_hint('读取技能记忆失败：${_skillMemories.error}'));
    } else if (!_skillMemoriesLoaded) {
      rows.add(_hint('正在读取…'));
    } else if (_skillMemories.items.isEmpty) {
      rows.add(
        _hint(
          '还没有技能记忆。开启「对话 → 更多设置 → 技能记忆」后，'
          '技能跑在子代理里记下的笔记会出现在这里。',
        ),
      );
    } else {
      for (final entry in _skillMemories.items) {
        rows.add(
          Row(
            children: [
              SizedBox(
                width: 160,
                child: Text(
                  entry.key,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(color: LunacColors.text, fontSize: 12),
                ),
              ),
              Expanded(
                child: Text(
                  '${entry.chars} 字符',
                  style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
                ),
              ),
              _settingsButton(
                '查看',
                () => unawaited(_showSkillMemory(entry.key)),
              ),
              const SizedBox(width: 6),
              _settingsButton(
                '清空',
                () => unawaited(_clearSkillMemory(entry.key)),
              ),
            ],
          ),
        );
      }
    }
    if (_skillMessage != null) {
      rows.add(
        Text(
          _skillMessage!,
          style: TextStyle(
            color: _skillMessageError
                ? LunacColors.danger
                : LunacColors.textDim,
            fontSize: 11,
          ),
        ),
      );
    }
    rows.add(
      Row(
        children: [
          _hint('改动后需重启 AI 才生效（会结束正在进行的回合）。'),
          const Spacer(),
          _settingsButton('重启 AI', _restartAgentForSkills),
        ],
      ),
    );
    return rows;
  }

  /// 「工具 (MCP)」组（v1 `#sp-ai` 的第三组）：远端 MCP 服务器配置的唯一入口。
  ///
  /// 为什么只给「打开配置文件」：agent 的 `config\mcp.json` 是 `{ "servers": [...] }`，
  /// 字段里既有 URL 也有静态头 / OAuth 选择，写错一个就让**整份**配置被忽略
  /// （`core-agent/mcp.rs::parse_config`）。与其做半吊子表单，不如把文件交到编辑器里。
  /// 项目级 `.mcp.json` 不在这里管 —— 它跟着工作区走，首次连接由对话里的信任卡把关。
  List<Widget> _buildMcpRows() {
    final rows = <Widget>[
      Row(
        children: [
          _label('远端服务器：<exe 根>\\config\\mcp.json'),
          const Spacer(),
          _settingsButton('打开配置', _openMcpConfig),
        ],
      ),
      _hint(
        '项目级 .mcp.json 由每个工作区自带，首次连接会弹信任卡（不在这一页配）。'
        '改完配置要「重启 AI」才生效；工具黑名单在「更多设置 → 权限」。',
      ),
    ];
    if (_mcpMessage != null) {
      rows.add(
        Text(
          _mcpMessage!,
          style: TextStyle(
            color: _mcpMessageError ? LunacColors.danger : LunacColors.textDim,
            fontSize: 11,
          ),
        ),
      );
    }
    return rows;
  }

  /// 打开（必要时先建）远端 MCP 配置文件；没有默认编辑器时如实给出路径。
  void _openMcpConfig() {
    final result = ensureMcpConfigFile();
    if (result.path.isEmpty) {
      setState(() {
        _mcpMessage = result.error;
        _mcpMessageError = true;
      });
      return;
    }
    final opened = openPath(result.path);
    setState(() {
      _mcpMessage = opened
          ? '已打开：${result.path}（改完点上面的「重启 AI」才生效）'
          : '配置文件在：${result.path}（系统没给出默认编辑器，请手动打开）';
      _mcpMessageError = !opened;
    });
  }

  Widget _buildSkillRow(SkillEntry skill) {
    return Row(
      children: [
        Icon(
          Icons.auto_awesome_outlined,
          size: 13,
          color: skill.valid ? LunacColors.textMuted : LunacColors.danger,
        ),
        const SizedBox(width: 6),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  Text(
                    skill.title,
                    style: TextStyle(
                      color: skill.valid
                          ? LunacColors.text
                          : LunacColors.danger,
                      fontSize: 12.5,
                    ),
                  ),
                  if (skill.fork) ...[
                    const SizedBox(width: 6),
                    _skillBadge('子代理', LunacColors.accent),
                  ],
                  if (!skill.valid) ...[
                    const SizedBox(width: 6),
                    _skillBadge('读不了', LunacColors.danger),
                  ],
                ],
              ),
              Text(
                skill.description.isEmpty
                    ? skill.key
                    : '${skill.key} · ${skill.description}',
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
              ),
            ],
          ),
        ),
        const SizedBox(width: 6),
        _settingsButton('编辑', () => _editSkill(skill)),
        const SizedBox(width: 4),
        _settingsButton('删除', () => _deleteSkill(skill)),
      ],
    );
  }

  Widget _skillBadge(String text, Color color) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 5, vertical: 1),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.16),
        borderRadius: BorderRadius.circular(4),
      ),
      child: Text(text, style: TextStyle(color: color, fontSize: 9.5)),
    );
  }

  /// 「定时任务（实验）」：只读列表 + 立即运行 + 逐 job 可写授权（v1 阶段 1 同口径）。
  List<Widget> _buildCronRows() {
    final rows = <Widget>[
      Row(
        children: [
          _label(_cron.schedulerAlive ? '调度器：运行中' : '调度器：未运行'),
          const Spacer(),
          _settingsButton('刷新', _refreshCron),
        ],
      ),
    ];
    if (!_cron.ok) {
      rows.add(
        _label(switch (_cron.reason) {
          'missing' => '还没有任务定义（ModuleData/tasks/cron.json 不存在）。',
          'invalid' => 'cron.json 解析失败 —— 宿主不会替你清空，请先修好它。',
          _ => '定时任务后端不可用（桥接未加载？）。',
        }),
      );
      rows.add(
        _hint(
          '定义写在 ModuleData/tasks/cron.json；agent 侧 CronCreate 需要可配置工作区'
          '（v2 尚未移植），先手工写文件也能被调度。',
        ),
      );
    } else if (_cron.jobs.isEmpty) {
      rows.add(_label('暂无定时任务。'));
      rows.add(
        _hint(
          '定义写在 ModuleData/tasks/cron.json（形如 '
          '{"version":1,"jobs":[{"id":"cron_x","prompt":"…",'
          '"schedule":{"kind":"every","everyMinutes":60}}]}）。',
        ),
      );
    } else {
      for (final job in _cron.jobs) {
        rows.add(
          Container(
            margin: const EdgeInsets.only(top: 10),
            padding: const EdgeInsets.only(top: 8),
            decoration: BoxDecoration(
              border: Border(top: BorderSide(color: LunacColors.border)),
            ),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Row(
                  children: [
                    Expanded(
                      child: Text(
                        job.scheduleText,
                        style: TextStyle(
                          color: LunacColors.text,
                          fontSize: 12.5,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                    ),
                    _settingsButton(
                      job.enabled ? '停用' : '启用',
                      _cronBusyId == null
                          ? () => _toggleCronEnabled(job)
                          : null,
                    ),
                    const SizedBox(width: 6),
                    _settingsButton(
                      _cronBusyId == job.id ? '运行中…' : '立即运行',
                      _cronBusyId == null ? () => _runCronJob(job) : null,
                    ),
                  ],
                ),
                if (job.prompt.isNotEmpty) _hint(job.prompt),
                _hint(
                  '${_cronStatusText(job)} · 已运行 ${job.runCount} 次 · '
                  '失败 ${job.failureCount} 次'
                  '${job.nextRunAtLocal.isEmpty ? '' : ' · 下次 ${job.nextRunAtLocal}'}'
                  ' · 今日 ${job.todayRuns}/${job.maxRunsPerDay}',
                ),
                if (job.lastError.isNotEmpty) _hint('上次错误：${job.lastError}'),
                Row(
                  children: [
                    _label('无人值守可写'),
                    const Spacer(),
                    _settingsButton(
                      _cronWriteConfirmId == job.id
                          ? (job.writeAccess ? '确认关闭？' : '确认开启？')
                          : (job.writeAccess ? '已开（点两次确认）' : '关（点两次确认）'),
                      () => _toggleCronWrite(job),
                    ),
                  ],
                ),
                _hint('开启后只放行 Write / Edit / NotebookEdit（写类三件）；命令类与联网工具照旧被裁。'),
              ],
            ),
          ),
        );
      }
    }
    final message = _cronMessage;
    if (message != null) {
      rows.add(
        Padding(
          padding: const EdgeInsets.only(top: 6),
          child: Text(
            message,
            style: TextStyle(
              color: _cronMessageError
                  ? LunacColors.danger
                  : LunacColors.textDim,
              fontSize: 11.5,
            ),
          ),
        ),
      );
    }
    return rows;
  }

  /// 「常规」分区：热键设置 + 开机自启（v1 `buildGeneralPane` 的前两行）。
  List<Widget> _buildGeneralRows() {
    final message = _hotkeyMessage;
    return [
      Row(
        children: [
          _label('热键设置'),
          const Spacer(),
          // v1 的 `.settings-hotkey`：圆角描边按钮本身就是录制入口（无独立「录制」按钮）。
          Tooltip(
            message: '点击录制新热键',
            child: Material(
              color: LunacColors.surfaceHover,
              borderRadius: BorderRadius.circular(6),
              child: InkWell(
                borderRadius: BorderRadius.circular(6),
                onTap: _toggleRecording,
                child: Container(
                  constraints: const BoxConstraints(minWidth: 90),
                  padding: const EdgeInsets.symmetric(
                    horizontal: 12,
                    vertical: 5,
                  ),
                  decoration: BoxDecoration(
                    borderRadius: BorderRadius.circular(6),
                    border: Border.all(
                      color: LunacColors.accent.withValues(alpha: 0.24),
                    ),
                  ),
                  alignment: Alignment.center,
                  child: Text(
                    _recordingHotkey ? '按下组合键…' : _hotkey,
                    style: TextStyle(
                      color: _recordingHotkey
                          ? LunacColors.accent
                          : LunacColors.text,
                      fontSize: 12,
                    ),
                  ),
                ),
              ),
            ),
          ),
        ],
      ),
      _hint('点按钮后按「修饰键 + 主键」（至少一个修饰键，例如 Ctrl+Alt+Space）；Esc 取消录制。'),
      if (message != null)
        Padding(
          padding: const EdgeInsets.only(top: 4),
          child: Text(
            message,
            style: TextStyle(
              color: _hotkeyError ? LunacColors.danger : LunacColors.textMuted,
              fontSize: 11,
            ),
          ),
        ),
      const SizedBox(height: 10),
      ..._buildAutoStartRows(),
    ];
  }

  /// 「开机自启」两行（状态 + 开关 / 修复）。
  ///
  /// 开发构建不允许注册（v1 的硬结论：会被开机项拉起成「弹黑窗 + 旧界面」），
  /// 所以开关直接禁用并写明原因，不做「点了报错」的空转按钮。
  List<Widget> _buildAutoStartRows() {
    final info = _autoStart;
    final message = _autoStartMessage;
    final rows = <Widget>[];
    if (info == null) {
      rows.add(
        Row(
          children: [
            _label('开机自启'),
            const Spacer(),
            Text(
              '不可用（rust 桥接未加载）',
              style: TextStyle(color: LunacColors.textDim, fontSize: 12),
            ),
          ],
        ),
      );
    } else if (!info.supported) {
      rows.add(
        Row(
          children: [
            _label('开机自启'),
            const Spacer(),
            Text(
              '当前平台暂不支持',
              style: TextStyle(color: LunacColors.textDim, fontSize: 12),
            ),
          ],
        ),
      );
    } else {
      // v1 的 `.settings-toggle`：只有一个开关，不写「开启 / 关闭」文字。
      final canToggle = !info.dev && !_autoStartBusy;
      rows.add(
        Row(
          children: [
            _label('开机自启'),
            const Spacer(),
            if (_autoStartBusy)
              Padding(
                padding: EdgeInsets.only(right: 8),
                child: Text(
                  '处理中…',
                  style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
                ),
              ),
            _V1Toggle(
              value: info.enabled,
              onChanged: canToggle ? (on) => _toggleAutoStart(on) : null,
            ),
          ],
        ),
      );
      if (info.stale && info.enabled) {
        rows.add(
          Row(
            children: [
              Expanded(
                child: Text(
                  '开机项指向旧程序（开机可能弹命令行窗口）',
                  style: TextStyle(color: LunacColors.textDim, fontSize: 11),
                ),
              ),
              _settingsButton(
                '修复',
                canToggle ? () => _toggleAutoStart(true) : null,
              ),
            ],
          ),
        );
      }
      if (info.dev) {
        rows.add(_hint('当前是开发构建，不注册开机自启（否则开机可能弹黑窗 / 界面是旧的）；请用安装版开启。'));
      }
      if (info.stale && info.enabled) {
        rows.add(_hint('已注册的开机项指向的不是当前程序：点「修复」重建（会弹一次管理员确认）。'));
      }
    }
    if (message != null) {
      rows.add(
        Padding(
          padding: const EdgeInsets.only(top: 4),
          child: Text(
            message,
            style: TextStyle(
              color: _autoStartError
                  ? LunacColors.danger
                  : LunacColors.textMuted,
              fontSize: 11,
            ),
          ),
        ),
      );
    }
    return rows;
  }

  /// 「网页搜索」分区（v1 `buildSearchPane`）：搜索引擎下拉 + 保存 + 已保存提示。
  Widget _buildEngineRow() {
    final selected = _pendingEngine ?? _engine;
    final message = _engineMessage;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            _label('搜索引擎'),
            const Spacer(),
            DropdownButtonHideUnderline(
              child: DropdownButton<String>(
                value: webEngines.containsKey(selected) ? selected : 'google',
                isDense: true,
                focusColor: Colors.transparent,
                dropdownColor: LunacColors.surfaceHover,
                borderRadius: BorderRadius.circular(6),
                icon: Icon(
                  Icons.expand_more,
                  size: 16,
                  color: LunacColors.textMuted,
                ),
                style: TextStyle(color: LunacColors.text, fontSize: 12),
                items: [
                  for (final entry in webEngines.entries)
                    DropdownMenuItem(
                      value: entry.key,
                      child: Text(entry.value),
                    ),
                ],
                onChanged: (value) {
                  if (value != null) {
                    _selectEngine(value);
                  }
                },
              ),
            ),
            const SizedBox(width: 8),
            _settingsButton('保存', _saveEngine),
            if (message != null)
              Flexible(
                child: Text(
                  message,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: _engineError
                        ? LunacColors.danger
                        : LunacColors.textMuted,
                    fontSize: 11,
                  ),
                ),
              ),
          ],
        ),
        _hint('右键点击启动器中的 "Web Search" 可快速切换引擎。'),
      ],
    );
  }

  /// 「风格 → 背景」：背景图来源 + 「自定义」四个滑块（v1 `buildAppearancePane` 第一段）。
  ///
  /// 背景图保存的是**盘上绝对路径**（v1 存 `convertFileSrc` 后的 asset URL）；
  /// 由搜索结果区 / AI 对话面板内部的 [AppearanceBackdropImage] 负责画（不再铺整窗）。
  List<Widget> _buildBackgroundRows() {
    final rows = <Widget>[];
    final path = _appearance.bgImage;
    final hasImage = path != null && path.isNotEmpty;
    final locked = _appearance.themeId != 'default';

    rows.add(
      Row(
        children: [
          _label('背景图片'),
          const Spacer(),
          Text(
            hasImage ? '已设置' : '未设置',
            style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
          ),
          const SizedBox(width: 8),
          _settingsButton('选择图片', locked ? null : _pickBackground),
          const SizedBox(width: 6),
          _settingsButton(
            _bgPanelOpen ? '自定义 ▴' : '自定义 ▾',
            () => setState(() => _bgPanelOpen = !_bgPanelOpen),
          ),
          const SizedBox(width: 6),
          _settingsButton('清除', hasImage ? _clearBackground : null),
        ],
      ),
    );

    if (_bgPanelOpen) {
      rows.add(
        _appearanceSliderRow(
          '毛玻璃化',
          _appearance.bgBlur,
          0,
          40,
          display: '${_appearance.bgBlur.round()}px',
          onChanged: (value) =>
              _previewAppearance(_appearance.copyWith(bgBlur: value)),
          onEnd: (value) => _persistAppearance({'bgBlur': value}),
        ),
      );
      rows.add(
        _appearanceSliderRow(
          '饱和度',
          _appearance.bgSaturate,
          0,
          200,
          display: '${_appearance.bgSaturate.round()}%',
          onChanged: (value) =>
              _previewAppearance(_appearance.copyWith(bgSaturate: value)),
          onEnd: (value) => _persistAppearance({'bgSaturate': value}),
        ),
      );
      rows.add(
        _appearanceSliderRow(
          '背景透明度',
          _appearance.bgOpacity,
          0,
          1,
          onChanged: (value) =>
              _previewAppearance(_appearance.copyWith(bgOpacity: value)),
          onEnd: (value) => _persistAppearance({'bgOpacity': value}),
        ),
      );
      rows.add(
        _appearanceSliderRow(
          '反光',
          _appearance.sheen,
          0,
          1,
          onChanged: (value) =>
              _previewAppearance(_appearance.copyWith(sheen: value)),
          onEnd: (value) => _persistAppearance({'sheen': value}),
        ),
      );
    }
    return rows;
  }

  /// 「风格 → 主题颜色」：恢复默认主题开关 + 四组「自定义 ▾」面板（v1 同序）。
  ///
  /// 配色只在「默认主题 + 未恢复默认」时可调；四条透明度滑块**不受锁定管辖**
  /// （v1 2026-10-18 裁定：透明度不是配色）。
  List<Widget> _buildColorRows() {
    final rows = <Widget>[];
    final locked = _appearance.themeId != 'default' || _appearance.tintBase;

    rows.add(
      Row(
        children: [
          _label('恢复默认主题'),
          const Spacer(),
          _V1Toggle(value: _appearance.tintBase, onChanged: _setTintBase),
        ],
      ),
    );
    if (_appearance.tintBase) {
      rows.add(_hint('已恢复默认主题：配色暂不可调；四种透明度已写回默认值，仍可继续调整。'));
    } else if (_appearance.themeId != 'default') {
      rows.add(_hint('当前主题已锁定「主题颜色」与「背景图片」，切回「默认」才能自定义'));
    }

    // ① 底色：取色 + 底色透明度 + 饱和度 / 明度。
    rows.add(
      _customPanelRow(
        '底色',
        '底色自定义',
        _basePanelOpen,
        () => setState(() => _basePanelOpen = !_basePanelOpen),
      ),
    );
    if (_basePanelOpen) {
      rows.add(
        _appearanceColorRow('底色', 'baseColor', _baseColor, enabled: !locked),
      );
      rows.add(
        _appearanceSliderRow(
          '底色透明度',
          _appearance.surfaceAlpha,
          0.3,
          1,
          onChanged: (value) =>
              _previewAppearance(_appearance.copyWith(surfaceAlpha: value)),
          onEnd: (value) => _persistAppearance({'surfaceAlpha': value}),
        ),
      );
      rows.add(_colorAxisRow('底色饱和度', 'baseColor', true, enabled: !locked));
      rows.add(_colorAxisRow('底色明度', 'baseColor', false, enabled: !locked));
    }

    // ② 按钮：线条（取色 + 透明度 + 饱和 / 明度）+ 背景（同）。
    rows.add(
      _customPanelRow(
        '按钮颜色',
        '按钮自定义',
        _btnPanelOpen,
        () => setState(() => _btnPanelOpen = !_btnPanelOpen),
      ),
    );
    if (_btnPanelOpen) {
      rows.add(
        _appearanceColorRow(
          '按钮线条',
          'btnLineColor',
          _btnLineColor,
          enabled: !locked,
        ),
      );
      rows.add(
        _appearanceSliderRow(
          '线条透明度',
          _appearance.btnLineAlpha,
          0,
          1,
          onChanged: (value) =>
              _previewAppearance(_appearance.copyWith(btnLineAlpha: value)),
          onEnd: (value) => _persistAppearance({'btnLineAlpha': value}),
        ),
      );
      rows.add(_colorAxisRow('线条饱和度', 'btnLineColor', true, enabled: !locked));
      rows.add(_colorAxisRow('线条明度', 'btnLineColor', false, enabled: !locked));
      rows.add(
        _appearanceColorRow(
          '按钮背景',
          'btnBgColor',
          _btnBgColor,
          enabled: !locked,
        ),
      );
      rows.add(
        _appearanceSliderRow(
          '背景透明度',
          _appearance.btnBgAlpha,
          0,
          1,
          onChanged: (value) =>
              _previewAppearance(_appearance.copyWith(btnBgAlpha: value)),
          onEnd: (value) => _persistAppearance({'btnBgAlpha': value}),
        ),
      );
      rows.add(_colorAxisRow('背景饱和度', 'btnBgColor', true, enabled: !locked));
      rows.add(_colorAxisRow('背景明度', 'btnBgColor', false, enabled: !locked));
    }

    // ③ 文字：只调三档文字的明度偏移。
    rows.add(
      _customPanelRow(
        '文字',
        '文字自定义',
        _textPanelOpen,
        () => setState(() => _textPanelOpen = !_textPanelOpen),
      ),
    );
    if (_textPanelOpen) {
      rows.add(
        _appearanceSliderRow(
          '文字明度',
          _appearance.textLight,
          -100,
          100,
          display: _appearance.textLight.toStringAsFixed(0),
          onChanged: (value) =>
              _previewAppearance(_appearance.copyWith(textLight: value)),
          onEnd: (value) => _persistAppearance({'textLight': value}),
        ),
      );
    }

    // ④ 按钮蒙版：一组取色 + 一条透明度（v1 2026-09-21 合并成「统一控制」）。
    rows.add(
      _customPanelRow(
        '按钮蒙版',
        '按钮蒙版自定义',
        _otherPanelOpen,
        () => setState(() => _otherPanelOpen = !_otherPanelOpen),
      ),
    );
    if (_otherPanelOpen) {
      rows.add(
        _appearanceColorRow(
          '蒙版颜色',
          'overlayColor',
          _overlayColor,
          enabled: !locked,
        ),
      );
      rows.add(
        _appearanceSliderRow(
          '蒙版透明度',
          _appearance.overlayAlpha,
          0,
          1,
          onChanged: (value) =>
              _previewAppearance(_appearance.copyWith(overlayAlpha: value)),
          onEnd: (value) => _persistAppearance({'overlayAlpha': value}),
        ),
      );
    }

    if (_appearanceMessage != null) {
      rows.add(
        Padding(
          padding: const EdgeInsets.only(top: 2),
          child: Text(
            _appearanceMessage!,
            style: TextStyle(
              color: _appearanceError
                  ? LunacColors.danger
                  : LunacColors.textMuted,
              fontSize: 11,
            ),
          ),
        ),
      );
    }
    rows.add(_hint('颜色留空 = 跟随主题；底色透明度在 v2 与原深底混合（没有桌面透视）。'));
    return rows;
  }

  /// 「风格 → 主题包」：主题目录入口 + 一排主题泡泡框（点一下立即切换）。
  List<Widget> _buildThemeRows() {
    final themes = _themes;
    final rows = <Widget>[
      Row(
        children: [
          _label('主题目录'),
          const Spacer(),
          _settingsButton('打开目录', _openThemesDir),
        ],
      ),
    ];

    if (themes == null) {
      rows.add(
        Text(
          '不可用（rust 桥接未加载）',
          style: TextStyle(color: LunacColors.textMuted, fontSize: 12),
        ),
      );
    } else if (themes.isEmpty) {
      rows.add(
        Text(
          '未找到主题包（把 theme.json 放进主题目录后重新打开设置）',
          style: TextStyle(color: LunacColors.textMuted, fontSize: 12),
        ),
      );
    } else {
      rows.add(const SizedBox(height: 6));
      rows.add(
        Wrap(
          spacing: 6,
          runSpacing: 6,
          children: [for (final theme in themes) _themeBubble(theme)],
        ),
      );
    }
    return rows;
  }

  /// 一行「标签 + 自定义 ▾」（v1 `.ap-bg-custom` 的展开按钮）。
  Widget _customPanelRow(
    String label,
    String buttonLabel,
    bool open,
    VoidCallback onTap,
  ) {
    return Row(
      children: [
        _label(label),
        const Spacer(),
        _settingsButton('$buttonLabel ${open ? '▴' : '▾'}', onTap),
      ],
    );
  }

  /// 选背景图：系统对话框 → 写 `bgImage` → 重套外观（背景层随色板版本重建）。
  void _pickBackground() {
    final path = pickBackgroundImage();
    if (path == null) {
      return;
    }
    final error = saveAppearanceConfig({'bgImage': path});
    if (error == null) {
      setState(() => _appearance = _appearance.copyWith(bgImage: path));
      _applyAppearanceLocal();
    }
    setState(() {
      _appearanceError = error != null;
      _appearanceMessage = error;
    });
  }

  /// 清除背景图（`bgImage: null`）。
  void _clearBackground() {
    final error = saveAppearanceConfig({'bgImage': null});
    if (error == null) {
      setState(() => _appearance = _appearance.copyWith(clearBgImage: true));
      _applyAppearanceLocal();
    }
    setState(() {
      _appearanceError = error != null;
      _appearanceMessage = error;
    });
  }

  /// v1「恢复默认主题」开关：打开时配色回主题自带值，四种透明度一次性写回默认值。
  void _setTintBase(bool on) {
    final patch = <String, Object?>{
      'tintBase': on,
      if (on) 'surfaceAlpha': 1.0,
      if (on) 'btnLineAlpha': 1.0,
      if (on) 'btnBgAlpha': 0.09,
      if (on) 'overlayAlpha': 0.1,
    };
    final error = saveAppearanceConfig(patch);
    if (error == null) {
      _appearance = _appearance.copyWith(
        tintBase: on,
        surfaceAlpha: on ? 1 : _appearance.surfaceAlpha,
        btnLineAlpha: on ? 1 : _appearance.btnLineAlpha,
        btnBgAlpha: on ? 0.09 : _appearance.btnBgAlpha,
        overlayAlpha: on ? 0.1 : _appearance.overlayAlpha,
      );
      _applyAppearanceLocal();
    }
    setState(() {
      _appearanceError = error != null;
      _appearanceMessage = error;
    });
  }

  /// 「跟随态」下取色器该显示的色（v1 `resolvedSwatches` 的 v2 等价物）。
  Color _resolvedSwatch(String field) {
    switch (field) {
      case 'baseColor':
        return LunacColors.background;
      case 'btnLineColor':
        return LunacColors.border;
      case 'btnBgColor':
        return LunacColors.accent;
      default:
        return LunacColors.divider;
    }
  }

  /// 当前槽位的实际颜色：用户写过就用用户的，否则用主题派生色。
  Color _currentSwatch(String field) {
    return parseThemeColor(_colorOf(field)) ?? _resolvedSwatch(field);
  }

  String _colorOf(String field) => switch (field) {
    'baseColor' => _appearance.baseColor,
    'btnLineColor' => _appearance.btnLineColor,
    'btnBgColor' => _appearance.btnBgColor,
    _ => _appearance.overlayColor,
  };

  /// 把颜色写回对应的输入框（HSV 轴滑块拖动时同步；避免回环不再触发输入框 onChanged）。
  void _syncColorController(String field, String hex) {
    final controller = switch (field) {
      'baseColor' => _baseColor,
      'btnLineColor' => _btnLineColor,
      'btnBgColor' => _btnBgColor,
      _ => _overlayColor,
    };
    if (controller.text != hex) {
      controller.text = hex;
    }
  }

  /// 一行「饱和度 / 明度」：从当前色取 HSV 轴，拖动即写回十六进制。
  Widget _colorAxisRow(
    String label,
    String field,
    bool saturation, {
    bool enabled = true,
  }) {
    final hsv = HSVColor.fromColor(_currentSwatch(field));
    final value = (saturation ? hsv.saturation : hsv.value) * 100;
    return _appearanceSliderRow(
      label,
      value,
      0,
      100,
      display: value.round().toString(),
      onChanged: enabled
          ? (next) {
              final base = HSVColor.fromColor(_currentSwatch(field));
              final updated = saturation
                  ? base.withSaturation((next / 100).clamp(0.0, 1.0))
                  : base.withValue((next / 100).clamp(0.0, 1.0));
              final hex = _hexOf(updated.toColor());
              _syncColorController(field, hex);
              _setAppearanceColor(field, hex, persist: false);
            }
          : null,
      onEnd: enabled
          ? (_) => _persistAppearance({field: _colorOf(field)})
          : null,
    );
  }

  /// `Color` → `#RRGGBB`（丢弃 alpha）。
  String _hexOf(Color color) {
    final rgb = color.toARGB32() & 0xFFFFFF;
    return '#${rgb.toRadixString(16).padLeft(6, '0').toUpperCase()}';
  }

  /// 按 id 找主题包（当前主题用于叠加自定义配色）。
  ThemePackage? _themeById(String id) {
    final themes = _themes;
    if (themes == null) {
      return null;
    }
    for (final theme in themes) {
      if (theme.id == id) {
        return theme;
      }
    }
    return null;
  }

  /// 把「主题包 + 自定义配色」重新套到运行时色板上（即时预览）。
  void _applyAppearanceLocal() {
    applyAppearance(_themeById(_appearance.themeId), _appearance);
  }

  /// 应用一个主题包：先落盘（失败就不改色板），成功后覆盖运行时色板并重建。
  void _applyTheme(ThemePackage theme) {
    final error = setAppearanceTheme(theme.id);
    if (error == null) {
      _appearance = _appearance.copyWith(themeId: theme.id);
      _applyAppearanceLocal();
    }
    setState(() {
      _appearanceTheme = error == null ? theme.id : _appearance.themeId;
      _appearanceError = error != null;
      _appearanceMessage =
          error ?? '已应用主题「${theme.name.isEmpty ? theme.id : theme.name}」';
    });
  }

  /// 校验并应用一个颜色（空串 = 跟随主题）。非法写法只提示、不入盘、不改色板。
  ///
  /// [persist] = false 用于滑块拖动（即时预览，松手时由 `onEnd` 统一写盘）。
  void _setAppearanceColor(String field, String raw, {bool persist = true}) {
    final text = raw.trim();
    final valid = text.isEmpty || RegExp(r'^#[0-9a-fA-F]{6}$').hasMatch(text);
    if (!valid) {
      setState(() {
        _appearanceError = true;
        _appearanceMessage = '颜色要写成 #RRGGBB（留空 = 跟随主题）';
      });
      return;
    }
    _appearance = _appearance.copyWith(
      baseColor: field == 'baseColor' ? text : null,
      btnLineColor: field == 'btnLineColor' ? text : null,
      btnBgColor: field == 'btnBgColor' ? text : null,
      overlayColor: field == 'overlayColor' ? text : null,
    );
    _applyAppearanceLocal();
    if (!persist) {
      return;
    }
    final error = saveAppearanceConfig({field: text});
    setState(() {
      _appearanceError = error != null;
      _appearanceMessage = error;
    });
  }

  /// 滑块即时预览（写盘只在松手时做，避免拖动过程刷爆磁盘）。
  void _previewAppearance(AppearanceConfig next) {
    setState(() => _appearance = next);
    _applyAppearanceLocal();
  }

  void _persistAppearance(Map<String, Object?> patch) {
    final error = saveAppearanceConfig(patch);
    if (error == null) {
      return;
    }
    setState(() {
      _appearanceError = true;
      _appearanceMessage = error;
    });
  }

  /// 一行「颜色」：标签 + 十六进制输入 + 色块 + 「跟随主题」。
  ///
  /// 输入框留空时色块显示该槽位的主题派生色（v1 `resolvedSwatches` 的口径）。
  /// 点色块 / ▾ 展开 v1 同款色板：饱和度-明度方块 + 色相条 + 预设色板。
  Widget _appearanceColorRow(
    String label,
    String field,
    TextEditingController controller, {
    bool enabled = true,
  }) {
    final dot = _currentSwatch(field);
    final open = _pickerOpen == field;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            SizedBox(width: 58, child: _label(label)),
            SizedBox(
              width: 96,
              height: 28,
              child: TextField(
                controller: controller,
                enabled: enabled,
                inputFormatters: [LengthLimitingTextInputFormatter(7)],
                onChanged: (value) => _commitHexInput(field, value),
                onTapOutside: (_) => _revertHexField(field, controller),
                style: TextStyle(color: LunacColors.text, fontSize: 11.5),
                cursorColor: LunacColors.accent,
                decoration: InputDecoration(
                  isDense: true,
                  filled: true,
                  fillColor: LunacColors.surface,
                  hintText: '跟随主题',
                  hintStyle: TextStyle(
                    color: LunacColors.textMuted,
                    fontSize: 11,
                  ),
                  contentPadding: const EdgeInsets.symmetric(
                    horizontal: 8,
                    vertical: 6,
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
            const SizedBox(width: 6),
            Tooltip(
              message: open ? '收起色板' : '展开色板',
              child: InkWell(
                onTap: enabled
                    ? () => setState(() => _pickerOpen = open ? null : field)
                    : null,
                borderRadius: BorderRadius.circular(6),
                child: Container(
                  width: 34,
                  height: 26,
                  decoration: BoxDecoration(
                    color: dot,
                    border: Border.all(color: LunacColors.border),
                    borderRadius: BorderRadius.circular(6),
                  ),
                ),
              ),
            ),
            const SizedBox(width: 4),
            SizedBox(
              width: 28,
              height: 26,
              child: TextButton(
                onPressed: enabled
                    ? () => setState(() => _pickerOpen = open ? null : field)
                    : null,
                style: TextButton.styleFrom(
                  foregroundColor: LunacColors.textDim,
                  padding: EdgeInsets.zero,
                  minimumSize: Size.zero,
                  shape: RoundedRectangleBorder(
                    borderRadius: BorderRadius.circular(6),
                    side: BorderSide(color: LunacColors.border),
                  ),
                ),
                child: Text(
                  open ? '▴' : '▾',
                  style: const TextStyle(fontSize: 10),
                ),
              ),
            ),
            const Spacer(),
            _settingsButton(
              '跟随主题',
              enabled
                  ? () {
                      controller.clear();
                      _pickerHue.remove(field);
                      _setAppearanceColor(field, '');
                    }
                  : null,
            ),
          ],
        ),
        if (open) _colorPickerPanel(field, enabled: enabled),
      ],
    );
  }

  /// v1 口径：输入框里**合法**才提交（打字中间态静默忽略）；空串 = 跟随主题。
  void _commitHexInput(String field, String raw) {
    var text = raw.trim();
    if (text.isEmpty) {
      if (_colorOf(field).isNotEmpty) {
        _pickerHue.remove(field);
        _setAppearanceColor(field, '');
      }
      return;
    }
    if (!text.startsWith('#')) {
      text = '#$text';
    }
    if (!RegExp(r'^#[0-9a-fA-F]{6}$').hasMatch(text)) {
      return;
    }
    final parsed = parseThemeColor(text);
    if (parsed != null) {
      _pickerHue[field] = HSVColor.fromColor(parsed).hue;
    }
    _setAppearanceColor(field, text);
  }

  /// 失焦时把非法输入回滚成已存值（v1 `paint(false)`）。
  void _revertHexField(String field, TextEditingController controller) {
    final stored = _colorOf(field);
    if (controller.text.trim() != stored) {
      controller.text = stored;
    }
  }

  /// 当前槽位色板的色相（用户拖过色相条就沿用那一档）。
  double _hueOf(String field) =>
      _pickerHue[field] ?? HSVColor.fromColor(_currentSwatch(field)).hue;

  /// v1 `.ap-pick-panel`：SV 方块 + 色相条 + 预设色板。
  Widget _colorPickerPanel(String field, {required bool enabled}) {
    return Padding(
      padding: const EdgeInsets.only(left: 58, top: 4),
      child: Container(
        key: ValueKey('appearance-picker-$field'),
        padding: const EdgeInsets.all(6),
        decoration: BoxDecoration(
          border: Border.all(color: LunacColors.border),
          borderRadius: BorderRadius.circular(8),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            _svSquare(field, enabled: enabled),
            const SizedBox(height: 4),
            _hueBar(field, enabled: enabled),
            const SizedBox(height: 5),
            _presetRow(field, enabled: enabled),
          ],
        ),
      ),
    );
  }

  /// 饱和度-明度方块（x = 饱和度，y = 明度；上亮下暗）。
  Widget _svSquare(String field, {required bool enabled}) {
    final hue = _hueOf(field);
    final hsv = HSVColor.fromColor(_currentSwatch(field));
    final saturation = hsv.saturation;
    final value = hsv.value;
    return LayoutBuilder(
      builder: (context, constraints) {
        final width = constraints.maxWidth;
        void apply(Offset local) {
          final s = (local.dx / width).clamp(0.0, 1.0);
          final v = (1 - local.dy / 80).clamp(0.0, 1.0);
          final hex = _hexOf(HSVColor.fromAHSV(1, hue, s, v).toColor());
          _pickerHue[field] = hue;
          _syncColorController(field, hex);
          _setAppearanceColor(field, hex, persist: false);
        }

        return GestureDetector(
          onPanDown: enabled ? (d) => apply(d.localPosition) : null,
          onPanUpdate: enabled ? (d) => apply(d.localPosition) : null,
          onPanEnd: enabled
              ? (_) => _persistAppearance({field: _colorOf(field)})
              : null,
          child: SizedBox(
            height: 80,
            child: Stack(
              children: [
                Positioned.fill(
                  child: DecoratedBox(
                    decoration: BoxDecoration(
                      color: HSVColor.fromAHSV(1, hue, 1, 1).toColor(),
                      borderRadius: BorderRadius.circular(6),
                    ),
                  ),
                ),
                Positioned.fill(
                  child: DecoratedBox(
                    decoration: BoxDecoration(
                      borderRadius: BorderRadius.circular(6),
                      gradient: LinearGradient(
                        colors: [Colors.white, Colors.white.withAlpha(0)],
                      ),
                    ),
                  ),
                ),
                Positioned.fill(
                  child: DecoratedBox(
                    decoration: BoxDecoration(
                      borderRadius: BorderRadius.circular(6),
                      gradient: LinearGradient(
                        begin: Alignment.topCenter,
                        end: Alignment.bottomCenter,
                        colors: [Colors.transparent, Colors.black],
                      ),
                    ),
                  ),
                ),
                Positioned(
                  left: saturation * width - 6,
                  top: (1 - value) * 80 - 6,
                  child: _pickerCursor(),
                ),
                Positioned.fill(
                  child: IgnorePointer(
                    child: DecoratedBox(
                      decoration: BoxDecoration(
                        borderRadius: BorderRadius.circular(6),
                        border: Border.all(color: LunacColors.border),
                      ),
                    ),
                  ),
                ),
              ],
            ),
          ),
        );
      },
    );
  }

  /// 色相条（0° 在最左，红 → 紫红）。
  Widget _hueBar(String field, {required bool enabled}) {
    final hue = _hueOf(field);
    return LayoutBuilder(
      builder: (context, constraints) {
        final width = constraints.maxWidth;
        void apply(Offset local) {
          final next = (local.dx / width).clamp(0.0, 1.0) * 360;
          final hsv = HSVColor.fromColor(_currentSwatch(field));
          _pickerHue[field] = next;
          final hex = _hexOf(
            HSVColor.fromAHSV(1, next, hsv.saturation, hsv.value).toColor(),
          );
          _syncColorController(field, hex);
          _setAppearanceColor(field, hex, persist: false);
        }

        return GestureDetector(
          onPanDown: enabled ? (d) => apply(d.localPosition) : null,
          onPanUpdate: enabled ? (d) => apply(d.localPosition) : null,
          onPanEnd: enabled
              ? (_) => _persistAppearance({field: _colorOf(field)})
              : null,
          child: SizedBox(
            height: 14,
            child: Stack(
              children: [
                Positioned.fill(
                  child: DecoratedBox(
                    decoration: BoxDecoration(
                      borderRadius: BorderRadius.circular(7),
                      gradient: LinearGradient(
                        colors: [
                          for (var i = 0; i <= 6; i++)
                            HSVColor.fromAHSV(1, i * 60.0, 1, 1).toColor(),
                        ],
                      ),
                    ),
                  ),
                ),
                Positioned(
                  left: (hue / 360) * width - 6,
                  top: 1,
                  child: _pickerCursor(),
                ),
                Positioned.fill(
                  child: IgnorePointer(
                    child: DecoratedBox(
                      decoration: BoxDecoration(
                        borderRadius: BorderRadius.circular(7),
                        border: Border.all(color: LunacColors.border),
                      ),
                    ),
                  ),
                ),
              ],
            ),
          ),
        );
      },
    );
  }

  /// 预设色板（v1 `colorPickerHtml` 里那 8 个）。
  Widget _presetRow(String field, {required bool enabled}) {
    return Wrap(
      spacing: 5,
      runSpacing: 4,
      children: [
        for (final hex in _appearancePresets)
          InkWell(
            key: ValueKey('appearance-preset-$hex'),
            onTap: enabled
                ? () {
                    final parsed = parseThemeColor(hex);
                    if (parsed != null) {
                      _pickerHue[field] = HSVColor.fromColor(parsed).hue;
                    }
                    _syncColorController(field, hex);
                    _setAppearanceColor(field, hex);
                  }
                : null,
            borderRadius: BorderRadius.circular(4),
            child: Container(
              width: 18,
              height: 18,
              decoration: BoxDecoration(
                color: parseThemeColor(hex) ?? LunacColors.surface,
                border: Border.all(color: LunacColors.border),
                borderRadius: BorderRadius.circular(4),
              ),
            ),
          ),
      ],
    );
  }

  Widget _pickerCursor() {
    return Container(
      width: 12,
      height: 12,
      decoration: BoxDecoration(
        shape: BoxShape.circle,
        border: Border.all(color: Colors.white, width: 2),
        boxShadow: [
          BoxShadow(
            color: Colors.black.withAlpha(130),
            blurRadius: 0,
            spreadRadius: 1,
          ),
        ],
      ),
    );
  }

  /// 一行紧凑滑块（v1 `appearanceSliderRow`）。`onChanged` 为 null = 禁用（配色被锁）。
  Widget _appearanceSliderRow(
    String label,
    double value,
    double min,
    double max, {
    ValueChanged<double>? onChanged,
    ValueChanged<double>? onEnd,
    String? display,
  }) {
    return Row(
      children: [
        SizedBox(width: 58, child: _label(label)),
        Expanded(
          child: SliderTheme(
            data: SliderTheme.of(context).copyWith(
              trackHeight: 2,
              thumbShape: const RoundSliderThumbShape(enabledThumbRadius: 6),
              overlayShape: SliderComponentShape.noOverlay,
              activeTrackColor: LunacColors.accent,
              inactiveTrackColor: LunacColors.border,
              thumbColor: LunacColors.accent,
            ),
            child: SizedBox(
              height: 22,
              child: Slider(
                value: value.clamp(min, max),
                min: min,
                max: max,
                onChanged: onChanged,
                onChangeEnd: onEnd,
              ),
            ),
          ),
        ),
        SizedBox(
          width: 42,
          child: Text(
            display ?? value.toStringAsFixed(2),
            textAlign: TextAlign.right,
            style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
          ),
        ),
      ],
    );
  }

  /// 一个主题泡泡框（v1 `.ap-theme`）：主色圆点 + 名称 + 内置徽标；点击即应用。
  Widget _themeBubble(ThemePackage theme) {
    final dot = parseThemeColor(theme.accent);
    final active = theme.id == _appearanceTheme;
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      child: GestureDetector(
        onTap: () => _applyTheme(theme),
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
          decoration: BoxDecoration(
            color: active ? LunacColors.surfaceHover : null,
            border: Border.all(
              color: active ? LunacColors.accent : LunacColors.border,
            ),
            borderRadius: BorderRadius.circular(999),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Container(
                width: 8,
                height: 8,
                decoration: BoxDecoration(
                  color: dot ?? LunacColors.textMuted,
                  shape: BoxShape.circle,
                ),
              ),
              const SizedBox(width: 6),
              Text(
                theme.name.isEmpty ? '默认' : theme.name,
                style: TextStyle(
                  color: active ? LunacColors.text : LunacColors.textDim,
                  fontSize: 12,
                ),
              ),
              if (theme.builtin && theme.id != 'default') ...[
                const SizedBox(width: 6),
                Container(
                  padding: const EdgeInsets.symmetric(
                    horizontal: 5,
                    vertical: 1,
                  ),
                  decoration: BoxDecoration(
                    borderRadius: BorderRadius.circular(999),
                    border: Border.all(color: LunacColors.border),
                  ),
                  child: Text(
                    '内置',
                    style: TextStyle(
                      color: LunacColors.textMuted,
                      fontSize: 10,
                    ),
                  ),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }

  /// 打开主题目录（Rust 侧不存在时会先创建，用户可直接往里放主题）。
  void _openThemesDir() {
    final dir = themesDirectory();
    if (dir != null && dir.isNotEmpty) {
      openPath(dir);
    }
  }

  /// 「AI 模型」分组（v1 `buildAIPane`）：提供商 / 模型 / 接口地址 / API 密钥 + 测试连通 + 保存。
  List<Widget> _buildAiRows() {
    final message = _aiMessage;
    return [
      // v1 `settings.curl_btn`「命令添加」：粘贴 curl → 解析 → 填好下面四项。
      Row(
        children: [
          _settingsButton('命令添加', _importCurl),
          const SizedBox(width: 8),
          Expanded(
            child: Text(
              '粘贴一段 curl，自动填好供应商 / 接口地址 / 密钥 / 模型',
              style: TextStyle(color: LunacColors.textMuted, fontSize: 10.5),
            ),
          ),
        ],
      ),
      const SizedBox(height: 8),
      // 提供商：v1 的自定义下拉（每项可 × 删除、末尾「— 自定义供应商 —」内联新增）。
      Row(
        children: [
          SizedBox(width: 58, child: _label('提供商')),
          const Spacer(),
          _AiDropdown(
            tooltip: '供应商',
            value: _aiProvider,
            label: providerPreset(_aiProvider).label,
            options: _providerOptions,
            customLabel: '— 自定义供应商 —',
            onPick: _selectAiProvider,
            onDelete: (value) => _deleteAiOption(provider: true, value: value),
            onCustom: (value) => _addCustomAiOption(provider: true, raw: value),
          ),
        ],
      ),
      const SizedBox(height: 6),
      _fieldRow(
        '模型',
        _aiModel,
        hint: '例如 deepseek-chat',
        trailing: _modelDropdown(),
      ),
      const SizedBox(height: 6),
      _fieldRow('接口地址', _aiUrl, hint: 'https://api.example.com'),
      const SizedBox(height: 6),
      _fieldRow(
        'API 密钥',
        _aiKey,
        hint: 'sk-…',
        obscure: !_aiKeyVisible,
        trailing: IconButton(
          tooltip: _aiKeyVisible ? '隐藏' : '显示',
          icon: Icon(
            _aiKeyVisible ? Icons.visibility_off : Icons.visibility,
            size: 15,
          ),
          color: LunacColors.textMuted,
          visualDensity: VisualDensity.compact,
          onPressed: () => setState(() => _aiKeyVisible = !_aiKeyVisible),
        ),
      ),
      const SizedBox(height: 8),
      Row(
        children: [
          _settingsButton(
            _aiTesting ? '测试中...' : '测试连通',
            _aiTesting ? null : _testAi,
          ),
          const SizedBox(width: 8),
          _settingsButton('保存', _saveAi),
          if (message != null)
            Expanded(
              child: Padding(
                padding: const EdgeInsets.only(left: 8),
                child: Text(
                  message,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: _aiError
                        ? LunacColors.danger
                        : LunacColors.textMuted,
                    fontSize: 11,
                  ),
                ),
              ),
            ),
        ],
      ),
      _hint('保存到 <exe 根>/config/ai.json；agent 只在启动时读配置，改完点对话面板的「新对话」生效。'),
    ];
  }

  /// 「用量与成本」分组（v1 `buildUsageCostSection`）：打开价表 / 更新价格 + 候选确认。
  ///
  /// v1 把「逐日用量表格 + 计价」放在主界面表盘的展开面板里（能在对话中实时刷新），
  /// 设置里只留这两个按钮与候选价确认 —— 这里按同一口径实现，不画第二份表格。
  List<Widget> _buildCostRows() {
    final rows = _pricing.hasPending
        ? pricingPreviewRows(_pricing.text, _pricing.pendingText)
        : const <({String model, String change})>[];
    return [
      Row(
        children: [
          _settingsButton('打开 pricing.json', _openPricing),
          const SizedBox(width: 8),
          _settingsButton('更新价格（让 agent 抓官方定价页）', _updatePrices),
        ],
      ),
      if (_costMessage != null)
        Padding(
          padding: const EdgeInsets.only(top: 4),
          child: Text(
            _costMessage!,
            style: TextStyle(
              color: _costError ? LunacColors.danger : LunacColors.textMuted,
              fontSize: 11,
            ),
          ),
        ),
      if (_pricing.hasPending) ...[
        const SizedBox(height: 8),
        _label('待确认的新价格'),
        const SizedBox(height: 2),
        if (rows.isEmpty)
          _hint('候选价格读不懂（不是合法 JSON）—— 请让 agent 重写候选文件，或点「放弃」。')
        else
          for (final row in rows)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 2),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  SizedBox(
                    width: 150,
                    child: Text(
                      row.model,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(color: LunacColors.text, fontSize: 12),
                    ),
                  ),
                  Expanded(
                    child: Text(
                      row.change,
                      style: TextStyle(
                        color: LunacColors.textMuted,
                        fontSize: 12,
                      ),
                    ),
                  ),
                ],
              ),
            ),
        const SizedBox(height: 6),
        Row(
          children: [
            _settingsButton('确认写入', _confirmPricing),
            const SizedBox(width: 8),
            _settingsButton('放弃', _discardPricing),
          ],
        ),
      ],
      _hint(
        '价格表在 <exe 根>/config/pricing.json（元 / 百万 token）；候选价写在沙箱盘，确认后才覆盖正式表。',
      ),
    ];
  }

  /// 一行「标签 + 输入框」；[trailing] 放显示 / 隐藏等小按钮。
  Widget _fieldRow(
    String label,
    TextEditingController controller, {
    String? hint,
    bool obscure = false,
    Widget? trailing,
  }) {
    return Row(
      children: [
        SizedBox(width: 58, child: _label(label)),
        Expanded(
          child: SizedBox(
            height: 30,
            child: TextField(
              controller: controller,
              obscureText: obscure,
              style: TextStyle(color: LunacColors.text, fontSize: 12),
              cursorColor: LunacColors.accent,
              decoration: InputDecoration(
                isDense: true,
                filled: true,
                fillColor: LunacColors.surface,
                contentPadding: const EdgeInsets.symmetric(
                  horizontal: 8,
                  vertical: 6,
                ),
                hintText: hint,
                hintStyle: TextStyle(
                  color: LunacColors.textMuted,
                  fontSize: 12,
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
        ),
        ?trailing,
      ],
    );
  }

  /// 「模型」输入框右侧的常用模型下拉（v1 的模型下拉；输入框仍可手输任意名字）。
  Widget _modelDropdown() {
    final current = _aiModel.text.trim();
    return _AiDropdown(
      tooltip: '常用模型',
      value: current,
      label: current.isEmpty ? '选择模型' : current,
      options: _modelOptions,
      customLabel: '— 自定义 —',
      width: 200,
      onPick: (value) => setState(() => _aiModel.text = value),
      onDelete: (value) => _deleteAiOption(provider: false, value: value),
      onCustom: (value) => _addCustomAiOption(provider: false, raw: value),
    );
  }

  List<Widget> _buildIndexRows() {
    final status = _indexStatus;
    final Widget statusText;
    if (status == null) {
      statusText = Text(
        '文件索引不可用（rust 桥接未加载）',
        style: TextStyle(color: LunacColors.textMuted, fontSize: 12),
      );
    } else if (_rebuilding || status.scanning || status.notLoaded) {
      statusText = Row(
        children: [
          SizedBox(
            width: 12,
            height: 12,
            child: CircularProgressIndicator(
              strokeWidth: 1.5,
              color: LunacColors.accent,
            ),
          ),
          SizedBox(width: 8),
          Text(
            '正在建立文件索引…',
            style: TextStyle(color: LunacColors.textMuted, fontSize: 12),
          ),
        ],
      );
    } else {
      statusText = Text(
        status.truncated
            ? '已索引 ${status.count} 项（已达上限，结果只覆盖部分目录）'
            : '已索引 ${status.count} 项',
        style: TextStyle(color: LunacColors.textDim, fontSize: 12),
      );
    }
    return [
      Row(
        children: [
          _label('状态'),
          const Spacer(),
          _settingsButton(
            '重建索引',
            _rebuilding || status == null ? null : _rebuild,
          ),
        ],
      ),
      statusText,
      if (status != null && status.roots.isNotEmpty)
        _hint('扫描范围：${status.roots.join('、')}'),
    ];
  }

  /// 「更新」两行（v1 `buildGeneralPane` 的软件更新 + 下载进度）：
  /// 状态文字 / 检查更新 / 下载并安装，进度与说明另起一行；末尾附数据目录。
  List<Widget> _buildUpdateRows() {
    final dir = _dataDir;
    final update = _update;
    final status = update?.status;
    final error = update?.error;

    // v1 的状态文字口径：没查过 =「当前版本 vX」；查过 =「有新版本 vX」/「已是最新版本」。
    final String statusText;
    if (_checkingUpdate) {
      statusText = '检查中…';
    } else if (status == null) {
      statusText = '当前版本 $kAppVersion';
    } else if (status.hasUpdate) {
      statusText = '有新版本 v${status.latest}';
    } else {
      statusText = '已是最新版本';
    }

    return [
      Row(
        children: [
          _label('更新'),
          const SizedBox(width: 12),
          Expanded(
            child: Text(
              statusText,
              textAlign: TextAlign.right,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(color: LunacColors.textDim, fontSize: 12),
            ),
          ),
          const SizedBox(width: 8),
          _settingsButton(
            _checkingUpdate ? '检查中…' : '检查更新',
            _checkingUpdate || _installing ? null : _checkUpdate,
          ),
          if (status != null && status.hasUpdate && !_installing) ...[
            const SizedBox(width: 8),
            _settingsButton(
              _confirmInstall ? '再点一次确认' : '下载并安装',
              _confirmInstall ? () => _installUpdate(status) : _armInstall,
            ),
          ],
        ],
      ),
      if (status != null && status.hasUpdate) ...[
        if (status.notes.isNotEmpty)
          Padding(
            padding: const EdgeInsets.only(top: 2),
            child: Text(
              status.notes,
              maxLines: 4,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
            ),
          ),
        if (_installing || _installMessage != null)
          Padding(
            padding: const EdgeInsets.only(top: 4),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                if (_installing && _installProgress != null)
                  Padding(
                    padding: const EdgeInsets.only(bottom: 3),
                    child: LinearProgressIndicator(
                      value: (_installProgress! / 100)
                          .clamp(0.0, 1.0)
                          .toDouble(),
                      minHeight: 2,
                      backgroundColor: LunacColors.border,
                      color: LunacColors.accent,
                    ),
                  ),
                Text(
                  _installMessage ?? '下载中…',
                  style: TextStyle(
                    color: _installError
                        ? LunacColors.danger
                        : LunacColors.textMuted,
                    fontSize: 11,
                  ),
                ),
              ],
            ),
          ),
      ],
      if (error != null)
        Padding(
          padding: const EdgeInsets.only(top: 2),
          child: Text(
            error,
            style: TextStyle(color: LunacColors.danger, fontSize: 11),
          ),
        ),
      Row(
        children: [
          _label('自动更新'),
          const Spacer(),
          _V1Toggle(value: _updateAuto, onChanged: _toggleUpdateAuto),
        ],
      ),
      _hint('开启后启动时自动检查，发现新版本会自动下载安装并重启应用。'),
      if (_updateAutoMessage != null)
        Padding(
          padding: const EdgeInsets.only(top: 2),
          child: Text(
            _updateAutoMessage!,
            style: TextStyle(
              color: _updateAutoError
                  ? LunacColors.danger
                  : LunacColors.textMuted,
              fontSize: 11,
            ),
          ),
        ),
      _hint('安装包会下载到 ${dir ?? '数据目录'}/temp/update；静默安装会退出应用并由安装器重新拉起。'),
      const SizedBox(height: 6),
      Row(
        children: [
          _label('数据目录'),
          const SizedBox(width: 12),
          Expanded(
            child: SelectableText(
              dir ?? '不可用',
              textAlign: TextAlign.right,
              maxLines: 1,
              style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
            ),
          ),
        ],
      ),
      _hint('配置 / 索引缓存 / 使用频次都在这个目录下（config、temp、data）。'),
    ];
  }
}

/// 市场里的一行（已装 + 索引合并后的形状）。
class _MarketRow {
  const _MarketRow({
    required this.id,
    required this.name,
    required this.description,
    required this.icon,
    required this.version,
    required this.installed,
    required this.valid,
    required this.error,
    required this.deps,
    required this.permissions,
    required this.url,
    required this.sha256,
    required this.update,
    this.hasSidecar = false,
  });

  final String id;
  final String name;
  final String description;
  final String icon;
  final String version;

  /// 盘上有目录（含坏包）。
  final bool installed;

  /// 清单与入口都通过校验。
  final bool valid;

  /// `valid == false` 时的原因。
  final String error;

  final int deps;
  final List<String> permissions;

  /// 清单声明了 `sidecar`（插件自带本机进程，可用任何语言写）。
  final bool hasSidecar;

  /// 索引给的 https zip 地址；空 = 索引里没有这条（只可能「卸载」）。
  final String url;
  final String sha256;

  /// `url` 非空且版本不同 ⇒ 按钮写「更新」。
  final bool update;
}

/// v1 `.settings-btn` / `.settings-install-btn` 的等价物：细描边 + 极浅底的小按钮。
///
/// v1 设置里的次按钮全都长这样（不是带强调色的文字按钮）；用同一个组件免得各处漂移。
class _SettingsButton extends StatelessWidget {
  const _SettingsButton({required this.label, this.onPressed});

  final String label;
  final VoidCallback? onPressed;

  @override
  Widget build(BuildContext context) {
    final enabled = onPressed != null;
    return Material(
      color: LunacColors.surfaceHover.withValues(alpha: enabled ? 1 : 0.6),
      borderRadius: BorderRadius.circular(6),
      child: InkWell(
        borderRadius: BorderRadius.circular(6),
        onTap: onPressed,
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(6),
            border: Border.all(color: LunacColors.border),
          ),
          child: Text(
            label,
            style: TextStyle(
              color: enabled ? LunacColors.textDim : LunacColors.textMuted,
              fontSize: 12,
            ),
          ),
        ),
      ),
    );
  }
}

/// 下拉的一项（值 + 展示名）。
class _AiOption {
  const _AiOption({required this.value, required this.label});

  final String value;
  final String label;
}

/// v1 `.custom-select` 的 Flutter 等价物（设置 → AI 的供应商 / 模型下拉）。
///
/// 与 Flutter 自带 `DropdownButton` 的差别就是 v1 的三件特有能力：
///   · 每项右侧 `×`：自定义项从名单里摘掉、内置项进隐藏名单；
///   · 末尾「— 自定义 —」：展开成**内联输入行**，回车 / 对勾写入并选中；
///   · 当前值不在候选里也照样显示（配置文件来自别处时不丢选中项）。
class _AiDropdown extends StatefulWidget {
  const _AiDropdown({
    required this.value,
    required this.label,
    required this.options,
    required this.customLabel,
    required this.onPick,
    required this.onDelete,
    required this.onCustom,
    this.tooltip,
    this.width = 180,
  });

  final String value;
  final String label;
  final List<_AiOption> options;
  final String customLabel;
  final ValueChanged<String> onPick;
  final ValueChanged<String> onDelete;
  final ValueChanged<String> onCustom;
  final String? tooltip;
  final double width;

  @override
  State<_AiDropdown> createState() => _AiDropdownState();
}

class _AiDropdownState extends State<_AiDropdown> {
  final MenuController _controller = MenuController();
  final TextEditingController _input = TextEditingController();
  bool _custom = false;

  @override
  void dispose() {
    _input.dispose();
    super.dispose();
  }

  void _resetCustom() {
    if (_custom) {
      setState(() => _custom = false);
    }
    _input.clear();
  }

  void _close() {
    _controller.close();
    _resetCustom();
  }

  void _commitCustom() {
    final value = _input.text.trim();
    if (value.isEmpty) {
      return;
    }
    widget.onCustom(value);
    _close();
  }

  @override
  Widget build(BuildContext context) {
    return MenuAnchor(
      controller: _controller,
      alignmentOffset: const Offset(0, 4),
      onClose: _resetCustom,
      style: MenuStyle(
        backgroundColor: WidgetStatePropertyAll(LunacColors.surfaceHover),
        padding: const WidgetStatePropertyAll(EdgeInsets.all(4)),
        shape: WidgetStatePropertyAll(
          RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(8),
            side: BorderSide(color: LunacColors.border),
          ),
        ),
      ),
      menuChildren: [
        if (_custom)
          SizedBox(
            width: widget.width,
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 4),
              child: Row(
                children: [
                  Expanded(
                    child: SizedBox(
                      height: 28,
                      child: TextField(
                        controller: _input,
                        autofocus: true,
                        onSubmitted: (_) => _commitCustom(),
                        style: TextStyle(color: LunacColors.text, fontSize: 12),
                        cursorColor: LunacColors.accent,
                        decoration: InputDecoration(
                          isDense: true,
                          hintText: '输入名称',
                          hintStyle: TextStyle(
                            color: LunacColors.textMuted,
                            fontSize: 11,
                          ),
                          filled: true,
                          fillColor: LunacColors.surface,
                          contentPadding: const EdgeInsets.symmetric(
                            horizontal: 8,
                            vertical: 6,
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
                  ),
                  IconButton(
                    tooltip: '确定',
                    icon: const Icon(Icons.check, size: 15),
                    color: LunacColors.accent,
                    visualDensity: VisualDensity.compact,
                    onPressed: _commitCustom,
                  ),
                ],
              ),
            ),
          )
        else ...[
          for (final option in widget.options)
            // 用 InkWell 而不是 MenuItemButton：后者被按下会**自动关菜单**，
            // 而 v1 的 × 删除后菜单要保持展开、末尾「— 自定义 —」要就地变输入行。
            InkWell(
              onTap: () {
                widget.onPick(option.value);
                _close();
              },
              borderRadius: BorderRadius.circular(6),
              child: Padding(
                padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 7),
                child: SizedBox(
                  width: widget.width,
                  child: Row(
                    children: [
                      SizedBox(
                        width: 16,
                        child: option.value == widget.value
                            ? Icon(
                                Icons.check,
                                size: 12,
                                color: LunacColors.accent,
                              )
                            : null,
                      ),
                      Expanded(
                        child: Text(
                          option.label,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                            color: option.value == widget.value
                                ? LunacColors.text
                                : LunacColors.textDim,
                            fontSize: 12,
                          ),
                        ),
                      ),
                      GestureDetector(
                        onTap: () => widget.onDelete(option.value),
                        child: Padding(
                          padding: const EdgeInsets.symmetric(horizontal: 4),
                          child: Icon(
                            Icons.close,
                            size: 12,
                            color: LunacColors.textMuted,
                          ),
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ),
          const Divider(height: 1),
          InkWell(
            onTap: () => setState(() => _custom = true),
            borderRadius: BorderRadius.circular(6),
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 7),
              child: SizedBox(
                width: widget.width,
                child: Text(
                  widget.customLabel,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(color: LunacColors.textDim, fontSize: 12),
                ),
              ),
            ),
          ),
        ],
      ],
      builder: (context, controller, child) {
        final button = InkWell(
          onTap: () =>
              controller.isOpen ? controller.close() : controller.open(),
          borderRadius: BorderRadius.circular(6),
          child: Container(
            padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 5),
            decoration: BoxDecoration(
              color: LunacColors.surface,
              border: Border.all(color: LunacColors.border),
              borderRadius: BorderRadius.circular(6),
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                ConstrainedBox(
                  constraints: BoxConstraints(maxWidth: widget.width - 48),
                  child: Text(
                    widget.label,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(color: LunacColors.text, fontSize: 12),
                  ),
                ),
                const SizedBox(width: 4),
                Icon(Icons.expand_more, size: 16, color: LunacColors.textMuted),
              ],
            ),
          ),
        );
        final tooltip = widget.tooltip;
        return tooltip == null
            ? button
            : Tooltip(message: tooltip, child: button);
      },
    );
  }
}

/// v1 `.settings-toggle` 的等价物：36×20 轨道 + 14px 圆点（选中时滑到右侧、线条加亮）。
class _V1Toggle extends StatelessWidget {
  const _V1Toggle({required this.value, this.onChanged});

  final bool value;

  /// null = 禁用（开发构建不允许注册开机自启 / 正在处理中）。
  final ValueChanged<bool>? onChanged;

  @override
  Widget build(BuildContext context) {
    final enabled = onChanged != null;
    final trackAlpha = enabled ? (value ? 0.44 : 0.31) : (value ? 0.22 : 0.18);
    return GestureDetector(
      onTap: enabled ? () => onChanged!(!value) : null,
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 180),
        width: 36,
        height: 20,
        padding: const EdgeInsets.all(3),
        decoration: BoxDecoration(
          color: LunacColors.accent.withValues(alpha: trackAlpha),
          borderRadius: BorderRadius.circular(20),
          border: Border.all(
            color: value
                ? LunacColors.accent.withValues(alpha: enabled ? 1 : 0.4)
                : Colors.transparent,
          ),
        ),
        child: AnimatedAlign(
          duration: const Duration(milliseconds: 180),
          alignment: value ? Alignment.centerRight : Alignment.centerLeft,
          child: Container(
            width: 14,
            height: 14,
            decoration: BoxDecoration(
              color: value ? LunacColors.accent : LunacColors.textDim,
              shape: BoxShape.circle,
            ),
          ),
        ),
      ),
    );
  }
}

// ── 插件安装进度的显示口径见 `platform/plugin_market.dart`
//（`percentText` / `formatByteSize` / `urlTail`，单测在 test/plugin_market_test.dart）──
