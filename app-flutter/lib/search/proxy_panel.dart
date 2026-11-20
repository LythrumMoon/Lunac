// 代理面板（v1 `proxy` 插件的原生等价物，docs/todo.md §待办 3-27 第三轮口径）。
//
// 三段式：
//   · 工具区：服务器导入 / 服务器订阅更新 / 设置（下拉 + 界面内独立面板）；
//   · 中间区：左抽屉（类型 / 名称 / 地址 / 端口 / 传输 / 来源订阅）+ 中央 168px 圆环
//     大字开关 + 上下行速率 + 出口地区（经引擎请求 ipinfo）+ 模式两档 + TUN 开关；
//   · 日志区：常驻展开、顶边可拖高，逐行显示「目标域名:端口 → 来源订阅 / 节点 / 直连 / 拒绝」。
//
// 能力边界（doc §禁用 1：不假装能力）：
//   · TUN 引擎未实现 —— 开关点了会如实报错并回弹，文案不写「已支持」；
//   · 自定义服务器（ss / trojan / vless / vmess / anytls / hysteria2 / tuic）的字段 → 分享链接由 Rust 侧
//     `lunac_proxy_custom_link` 拼装，并**用订阅同一个 `parse_link` 回读校验**；
//     Dart 不复制协议编码（tuic 的表单是 UUID + 密码两个框）。
//   · 出口地区是**经当前节点**请求 `ipinfo.io/country`，失败就显示原因，不拿本机冒充。
import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../platform/plugin_kv.dart';
import '../platform/proxy.dart';
import '../theme.dart';

/// 规则名单的默认值（v1 `mihomo_config` 的国内默认清单同口径：黑名单 = 名单内直连）。
const List<String> kDefaultRuleDomains = [
  'cn',
  'baidu.com',
  'bdstatic.com',
  'qq.com',
  'gtimg.com',
  'qpic.cn',
  'weixin.qq.com',
  'taobao.com',
  'tmall.com',
  'alicdn.com',
  'alipay.com',
  'aliyun.com',
  'jd.com',
  '360buyimg.com',
  'bilibili.com',
  'hdslb.com',
  'bilivideo.com',
  'douyin.com',
  'douyincdn.com',
  'bytedance.com',
  'toutiao.com',
  'ixigua.com',
  'kuaishou.com',
  'zhihu.com',
  'zhimg.com',
  '163.com',
  '126.net',
  'netease.com',
  'sina.com.cn',
  'weibo.com',
  'sinaimg.cn',
  'sohu.com',
  'iqiyi.com',
  'youku.com',
  'qqmusic.qq.com',
  'kugou.com',
  'xiami.com',
  '12306.cn',
  'ctrip.com',
  'meituan.com',
  'dianping.com',
  'pinduoduo.com',
  'xiaohongshu.com',
  'csdn.net',
  'gitee.com',
  'aliyuncs.com',
  'myqcloud.com',
  'qcloud.com',
  'tencentcloudapi.com',
  'huaweicloud.com',
  'mi.com',
  'xiaomi.com',
  'oppo.com',
  'vivo.com',
];

/// 插件存储的 id（与 v1 的插件目录名一致）。
const String _kvId = 'proxy';
const String _kvConfigKey = 'config';
const String _kvLogHeightKey = 'log_height';

/// 日志区默认高度 / 可拖范围（与 v1 的 190px 起始、至少给中间区留 320px 同口径）。
const double _logDefaultHeight = 190;
const double _logMinHeight = 96;
const double _logMaxHeight = 420;

/// 面板配置（本地存储的一条 JSON）。
///
/// 纯数据：`linksText` 是用户粘贴的订阅 / 分享链接原文，引擎启动时再拆
/// （[splitProxyInput]）；`ruleMode` / `ruleDomains` 展开成插件名单层（Rust 侧
/// `rules::domain_rules`，名单层排在订阅规则之前）。
class ProxyPanelConfig {
  ProxyPanelConfig({
    this.linksText = '',
    this.port = 7890,
    this.mode = 'rule',
    this.ruleMode = 'blacklist',
    List<String>? ruleDomains,
    List<String>? extraRules,
  }) : ruleDomains = List<String>.of(ruleDomains ?? kDefaultRuleDomains),
       extraRules = List<String>.of(extraRules ?? const []);

  String linksText;
  int port;

  /// `rule` 规则模式 / `global` 全局代理（面板只提供这两档）。
  String mode;

  /// `blacklist`（名单内直连）/ `whitelist`（名单内走代理）。
  String ruleMode;
  List<String> ruleDomains;

  /// 用户自己写的路由规则（一行一条，Clash 语法；排在名单层之后、订阅规则之前）。
  List<String> extraRules;

  /// 设置页里那一份「一行一个」的清单文本。
  String get domainsText => ruleDomains.join('\n');

  /// 自定义规则的文本形态（保存 / 回填用）。
  String get rulesText => extraRules.join('\n');

  /// 把清单文本写回（`#` / `//` 行与空行忽略，与 Rust 侧同一口径）。
  void setDomainsFromText(String text) {
    ruleDomains = [
      for (final line in text.split(RegExp(r'\r?\n')))
        if (line.trim().isNotEmpty &&
            !line.trim().startsWith('#') &&
            !line.trim().startsWith('//'))
          line.trim(),
    ];
  }

  /// 把自定义规则文本写回（注释 / 空行忽略；语法校验仍在 Rust 的 `rules::parse_rule`）。
  void setExtraRulesFromText(String text) {
    extraRules = [
      for (final line in text.split(RegExp(r'\r?\n')))
        if (line.trim().isNotEmpty &&
            !line.trim().startsWith('#') &&
            !line.trim().startsWith('//'))
          line.trim(),
    ];
  }

  Map<String, dynamic> toJson() => {
    'linksText': linksText,
    'port': port,
    'mode': mode,
    'ruleMode': ruleMode,
    'ruleDomains': ruleDomains,
    'extraRules': extraRules,
  };

  ProxyPanelConfig copy() => ProxyPanelConfig(
    linksText: linksText,
    port: port,
    mode: mode,
    ruleMode: ruleMode,
    ruleDomains: ruleDomains,
    extraRules: extraRules,
  );
}

/// 解析本地存储里的配置（纯函数，单测不加载 DLL）。
///
/// 坏数据 / 缺字段一律回默认值 —— 面板不因为一条脏记录就打不开。
ProxyPanelConfig parseProxyPanelConfig(Object? json) {
  if (json is! Map) {
    return ProxyPanelConfig();
  }
  final linksText = json['linksText'];
  final port = json['port'];
  final mode = json['mode'];
  final ruleMode = json['ruleMode'];
  final domains = json['ruleDomains'];
  final parsedDomains = [
    for (final item in (domains is List ? domains : const []))
      if (item is String && item.trim().isNotEmpty) item.trim(),
  ];
  final rules = json['extraRules'];
  final parsedRules = [
    for (final item in (rules is List ? rules : const []))
      if (item is String && item.trim().isNotEmpty) item.trim(),
  ];
  return ProxyPanelConfig(
    linksText: linksText is String ? linksText : '',
    port: port is int && port > 0 && port <= 65535 ? port : 7890,
    mode: mode == 'global' ? 'global' : 'rule',
    ruleMode: ruleMode == 'whitelist' ? 'whitelist' : 'blacklist',
    // 空清单 = 用户把默认清单删干净了？缺字段才回默认（两者语义不同）。
    ruleDomains: domains is List ? parsedDomains : kDefaultRuleDomains,
    // 自定义规则没有默认值：缺字段 / 空数组都等于「没有」。
    extraRules: parsedRules,
  );
}

/// 把输入框拆成「分享链接」与「整段订阅」（可混着贴）。
///
/// Clash / mihomo 的 YAML 是**多行结构**，按行拆再拼起来会把它拆坏（`proxies:` 与
/// `- name:` 被粘成一行），所以只要看到 `proxies:` 就整段原样交给引擎。
({List<String> links, String subscription}) splitProxyInput(String text) {
  final isClash = text
      .split(RegExp(r'\r?\n'))
      .any((line) => line.trimLeft().startsWith('proxies:'));
  if (isClash) {
    return (links: const <String>[], subscription: text.trim());
  }
  final lines = text
      .split(RegExp(r'\r?\n'))
      .map((line) => line.trim())
      .where((line) => line.isNotEmpty)
      .toList();
  return (
    links: [
      for (final line in lines)
        if (line.contains('://')) line,
    ],
    subscription: [
      for (final line in lines)
        if (!line.contains('://')) line,
    ].join(),
  );
}

/// 代理面板：搜索「代理 / 系统代理 / proxy」进入。
class ProxyPanel extends StatefulWidget {
  const ProxyPanel({super.key, required this.onClose, this.windowControls});

  /// 关闭面板（Esc / 返回按钮）。
  final VoidCallback onClose;

  /// 独立窗的「置顶 / 最小化 / 关闭」（由 `plugin_window_main` 注入）。
  /// 非 null ⇒ 替换标题栏里的「返回」；null ⇒ 内嵌模式，保留返回。
  final Widget? windowControls;

  @override
  State<ProxyPanel> createState() => _ProxyPanelState();
}

/// 工具区下拉里当前展开的界面内面板。
enum _ToolPanel { none, import, settings }

class _ProxyPanelState extends State<ProxyPanel> {
  late final FocusNode _panelFocus = FocusNode(
    debugLabel: 'proxy-panel',
    onKeyEvent: _onKey,
  );

  /// 状态；null = rust 桥接不可用。
  ProxyStatus? _status;

  /// 本地引擎状态（`lunac_proxy_local_status`）。
  LocalProxyStatus? _engine;

  /// 面板配置（启动参数 / 规则名单 / 端口）。
  ProxyPanelConfig _config = ProxyPanelConfig();

  /// 当前展开的工具面板。
  _ToolPanel _tool = _ToolPanel.none;

  /// 左抽屉是否展开（v1 口径：默认收起，展开时铺满中间区）。
  bool _drawerOpen = false;

  /// 已测过的节点时延（节点下标 → 展示文案）。
  final Map<int, String> _delays = {};

  /// 请求日志（最近 200 条，2s 拉一次）。
  List<ProxyRequestEntry> _log = const [];

  /// 日志区高度（可拖；写回本地存储）。
  double _logHeight = _logDefaultHeight;

  /// 上下行速率（字节/秒），由两次采样的累计值做差得到。
  int _upRate = 0;
  int _downRate = 0;
  int _lastUp = 0;
  int _lastDown = 0;
  DateTime? _lastSampleAt;

  /// 出口地区（两个字母；空 = 还没探测或失败）。
  String _country = '';
  String _countryError = '';
  bool _exitLoading = false;

  /// 服务器导入面板里的编辑框。
  late final TextEditingController _importController = TextEditingController();

  /// 自定义服务器表单（v1 `mihomo_node_add_custom` 的入口）。
  String _customKind = 'ss';
  late final TextEditingController _customName = TextEditingController();
  late final TextEditingController _customServer = TextEditingController();
  late final TextEditingController _customPort = TextEditingController(
    text: '8388',
  );
  late final TextEditingController _customSecret = TextEditingController();
  /// TUIC 的第二个凭据（v5 = uuid + 密码；其余协议只用到上面那个框）。
  late final TextEditingController _customUuid = TextEditingController();
  late final TextEditingController _customMethod = TextEditingController(
    text: 'aes-256-gcm',
  );
  /// 自定义服务器是否走 Xray 的 `mux.cool`（只对 trojan / vless / vmess 有意义）。
  bool _customMux = false;

  /// 自定义服务器最近一次「加入清单」的回执（成功 / 失败都给一句）。
  String _customNote = '';

  /// 设置面板里的编辑框（端口 / 域名清单）。
  late final TextEditingController _portController = TextEditingController();
  late final TextEditingController _domainsController = TextEditingController();
  /// 自定义规则（一行一条；语法由 Rust 的 `rules::parse_rule` 判定）。
  late final TextEditingController _rulesController = TextEditingController();

  bool _busy = false;

  /// 最近一次动作失败的原因。
  String? _actionError;

  final ScrollController _logScroll = ScrollController();
  Timer? _poll;

  @override
  void initState() {
    super.initState();
    _status = readProxyStatus();
    _engine = readLocalProxyStatus();
    _loadLocalConfig();
    _importController.text = _config.linksText;
    _portController.text = '${_config.port}';
    _domainsController.text = _config.domainsText;
    _rulesController.text = _config.rulesText;
    _log = readProxyRequests(limit: 200);
    _sampleTraffic(force: true);
    _poll = Timer.periodic(const Duration(seconds: 2), (_) => _pollTick());
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) {
        _panelFocus.requestFocus();
        _scrollLogToEnd();
      }
    });
  }

  @override
  void dispose() {
    _poll?.cancel();
    _logScroll.dispose();
    _panelFocus.dispose();
    _importController.dispose();
    _customName.dispose();
    _customServer.dispose();
    _customPort.dispose();
    _customSecret.dispose();
    _customUuid.dispose();
    _customMethod.dispose();
    _portController.dispose();
    _domainsController.dispose();
    _rulesController.dispose();
    super.dispose();
  }

  void _loadLocalConfig() {
    final stored = readPluginKv(_kvId, _kvConfigKey);
    if (stored.ok && stored.found) {
      _config = parseProxyPanelConfig(stored.value);
    }
    final height = readPluginKv(_kvId, _kvLogHeightKey);
    final value = height.ok && height.found ? height.value : null;
    if (value is num) {
      _logHeight = value.toDouble().clamp(_logMinHeight, _logMaxHeight);
    }
  }

  void _saveConfig(ProxyPanelConfig config) {
    _config = config;
    writePluginKv(id: _kvId, key: _kvConfigKey, value: config.toJson());
  }

  KeyEventResult _onKey(FocusNode node, KeyEvent event) {
    if (event is KeyDownEvent &&
        event.logicalKey == LogicalKeyboardKey.escape) {
      widget.onClose();
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  /// 每 2s：刷引擎状态 + 请求日志 + 速率（v1 同节奏）。
  void _pollTick() {
    if (!mounted) {
      return;
    }
    final engine = readLocalProxyStatus();
    final requests = readProxyRequests(limit: 200);
    setState(() {
      _engine = engine;
      _log = requests;
      _sampleTraffic();
    });
    _scrollLogToEnd();
  }

  /// 用累计上下行字节算速率（第一次采样没有基准 ⇒ 速率 0）。
  void _sampleTraffic({bool force = false}) {
    final engine = _engine;
    if (engine == null) {
      _upRate = 0;
      _downRate = 0;
      return;
    }
    final now = DateTime.now();
    if (force || _lastSampleAt == null) {
      _lastUp = engine.trafficUp;
      _lastDown = engine.trafficDown;
      _lastSampleAt = now;
      _upRate = 0;
      _downRate = 0;
      return;
    }
    final seconds = now.difference(_lastSampleAt!).inMilliseconds / 1000.0;
    if (seconds <= 0) {
      return;
    }
    final up = engine.trafficUp - _lastUp;
    final down = engine.trafficDown - _lastDown;
    _upRate = up < 0 ? 0 : (up / seconds).round();
    _downRate = down < 0 ? 0 : (down / seconds).round();
    _lastUp = engine.trafficUp;
    _lastDown = engine.trafficDown;
    _lastSampleAt = now;
  }

  void _scrollLogToEnd() {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || !_logScroll.hasClients) {
        return;
      }
      // 只有停在底部时才自动跟随（用户往上翻历史时不打断）。
      final position = _logScroll.position;
      if (position.maxScrollExtent - position.pixels < 48) {
        _logScroll.jumpTo(position.maxScrollExtent);
      }
    });
  }

  bool get _running => _engine?.running == true;

  List<ProxyNode> get _nodes => _engine?.nodes ?? const [];

  int get _active => _engine?.active ?? 0;

  /// 动作结果的统一落地：成功就用应答里的状态，失败就说原因。
  void _applyLocalReply(LocalProxyReply reply, {String fallback = ''}) {
    setState(() {
      _busy = false;
      final status = reply.status;
      if (status != null) {
        _engine = status;
      } else {
        _engine = readLocalProxyStatus() ?? _engine;
      }
      if (!reply.ok) {
        final reason = reply.error.trim();
        _actionError = reason.isNotEmpty
            ? reason
            : (fallback.isEmpty ? '操作没有成功，请重试' : fallback);
      } else {
        _actionError = null;
      }
    });
    _sampleTraffic(force: true);
  }

  /// 中央大按钮：开启 = 启引擎 + 打开系统代理；关闭 = 停引擎 + 还原系统代理。
  Future<void> _toggleService() async {
    if (_busy) {
      return;
    }
    if (_running) {
      await _stopService();
    } else {
      await _startService();
    }
  }

  Future<void> _startService() async {
    final input = splitProxyInput(_config.linksText);
    if (input.links.isEmpty && input.subscription.isEmpty) {
      setState(() {
        _tool = _ToolPanel.import;
        _actionError = '先导入服务器：粘贴订阅或分享链接（ss:// / trojan:// / vless:// / vmess:// / anytls:// / hysteria2:// / tuic://）';
      });
      return;
    }
    setState(() {
      _busy = true;
      _actionError = null;
      _delays.clear();
      _country = '';
      _countryError = '';
    });
    final reply = startLocalProxy(
      links: input.links,
      subscription: input.subscription,
      port: _config.port.clamp(1, 65535),
      mode: _config.mode,
      ruleMode: _config.ruleMode,
      ruleDomains: _config.ruleDomains,
      rules: _config.extraRules,
    );
    if (!mounted) {
      return;
    }
    _applyLocalReply(reply, fallback: '引擎没有启动成功');
    final engine = _engine;
    if (!reply.ok || engine == null || engine.port <= 0) {
      return;
    }
    // 与 v1 同口径：中间大按钮一并启停系统代理。
    final system = setSystemProxy(
      enabled: true,
      server: 'http://127.0.0.1:${engine.port}',
    );
    if (!mounted) {
      return;
    }
    setState(() {
      final actual = system.system;
      if (actual != null) {
        _status = ProxyStatus(
          supported: _status?.supported ?? true,
          system: actual,
          local: _status?.local ?? const ProxyLocalState(),
          kernel: _status?.kernel ?? const ProxyKernelState(),
          backup: system.ok ? true : (_status?.backup ?? false),
        );
      }
      if (!system.ok) {
        _actionError = system.error.trim().isEmpty
            ? '引擎已启动，但系统代理没有切过去'
            : '引擎已启动；${system.error.trim()}';
      }
    });
  }

  Future<void> _stopService() async {
    setState(() {
      _busy = true;
      _actionError = null;
    });
    final reply = stopLocalProxy();
    final system = setSystemProxy(enabled: false);
    if (!mounted) {
      return;
    }
    _applyLocalReply(reply, fallback: '引擎没有停掉');
    setState(() {
      final actual = system.system;
      if (actual != null) {
        _status = ProxyStatus(
          supported: _status?.supported ?? true,
          system: actual,
          local: _status?.local ?? const ProxyLocalState(),
          kernel: _status?.kernel ?? const ProxyKernelState(),
          backup: false,
        );
      }
      _upRate = 0;
      _downRate = 0;
      _country = '';
      _countryError = '';
      if (!system.ok && _actionError == null) {
        _actionError = system.error.trim().isEmpty
            ? '系统代理没有还原'
            : system.error.trim();
      }
    });
  }

  Future<void> _selectNode(int index) async {
    if (_busy) {
      return;
    }
    setState(() {
      _busy = true;
      _actionError = null;
    });
    final reply = selectLocalProxyNode(index);
    if (!mounted) {
      return;
    }
    _applyLocalReply(reply, fallback: '节点没有切过去');
    // 换了出口 ⇒ 旧的地区结论作废。
    setState(() {
      _country = '';
      _countryError = '';
    });
  }

  Future<void> _probeNode(int index) async {
    setState(() => _delays[index] = '测速中…');
    final reply = probeLocalProxyDelay(index);
    if (!mounted) {
      return;
    }
    setState(() {
      if (reply.ok) {
        _delays[index] = reply.scope == 'http'
            ? '${reply.ms} ms（经节点）'
            : '${reply.ms} ms（TCP）';
      } else {
        _delays[index] = reply.error.trim().isEmpty ? '测速失败' : '失败';
        _actionError = reply.error.trim().isEmpty ? null : reply.error.trim();
      }
    });
  }

  Future<void> _switchMode(String mode) async {
    if (_config.mode == mode) {
      return;
    }
    final config = _config.copy()..mode = mode;
    _saveConfig(config);
    if (!_running) {
      setState(() => _actionError = null);
      return;
    }
    final reply = setLocalProxyMode(mode);
    if (!mounted) {
      return;
    }
    _applyLocalReply(reply, fallback: '模式没有切过去');
  }

  Future<void> _probeExit() async {
    if (_exitLoading || !_running) {
      return;
    }
    setState(() {
      _exitLoading = true;
      _countryError = '';
    });
    final reply = probeExitCountry();
    if (!mounted) {
      return;
    }
    setState(() {
      _exitLoading = false;
      if (reply.ok && reply.country.isNotEmpty) {
        _country = reply.country;
        _countryError = '';
      } else {
        _country = '';
        _countryError = reply.error.trim().isEmpty
            ? '出口探测没有拿到结果'
            : reply.error.trim();
      }
    });
  }

  /// TUN 不是「已支持」：点了如实说明并保持关闭（doc §待办 3-27 未做清单）。
  void _tapTun(bool wanted) {
    if (!wanted) {
      return;
    }
    setState(
      () => _actionError =
          'TUN 本版未实现：需要 wintun 驱动 + 用户态 TCP/IP 栈，并且要先以管理员权限重启。'
          '在此之前请用系统代理（规则模式 / 全局代理）。',
    );
  }

  /// 「服务器订阅更新」：引擎运行时不允许热更新（监听已按旧规则建立）。
  void _refreshSubscription() {
    if (_running) {
      setState(() => _actionError = '订阅更新要先停用服务（引擎按启动时的订阅建监听）');
      return;
    }
    // 重新解析 = 用当前订阅把引擎拉起来（与 v1 的「刷新服务器」同一效果）。
    _startService();
  }

  /// 从剪贴板把订阅 / 分享链接贴进导入面板。
  Future<void> _importFromClipboard() async {
    final data = await Clipboard.getData(Clipboard.kTextPlain);
    if (!mounted) {
      return;
    }
    final text = data?.text?.trim() ?? '';
    if (text.isEmpty) {
      setState(() => _actionError = '剪贴板里没有文本');
      return;
    }
    setState(() {
      _importController.text = text;
      _actionError = null;
    });
  }

  void _saveImport() {
    final text = _importController.text.trim();
    if (text.isEmpty) {
      setState(() => _actionError = '先粘贴订阅或分享链接');
      return;
    }
    final config = _config.copy()..linksText = text;
    _saveConfig(config);
    setState(() {
      _tool = _ToolPanel.none;
      _actionError = _running ? '已保存；重启服务后生效' : null;
    });
  }

  /// 自定义服务器：字段 → 分享链接（Rust 拼 + 用订阅同一个解析器校验）→ 追加进导入框。
  void _addCustomServer() {
    final port = int.tryParse(_customPort.text.trim()) ?? 0;
    // vless / vmess 的凭据是 UUID；tuic 是 uuid + 密码；其余协议只有密码。
    final uuidKind = _customKind == 'vless' || _customKind == 'vmess';
    final tuic = _customKind == 'tuic';
    final reply = buildCustomProxyLink(
      kind: _customKind,
      name: _customName.text,
      server: _customServer.text,
      port: port,
      password: tuic || !uuidKind ? _customSecret.text : '',
      method: _customKind == 'ss' ? _customMethod.text : '',
      uuid: tuic
          ? _customUuid.text
          : (uuidKind ? _customSecret.text : ''),
      mux: _customMux,
    );
    if (!mounted) {
      return;
    }
    if (!reply.ok) {
      setState(() {
        _customNote = reply.error.trim().isEmpty ? '没有生成成功' : reply.error.trim();
      });
      return;
    }
    final existing = _importController.text.trim();
    _importController.text = existing.isEmpty
        ? reply.link
        : '$existing\n${reply.link}';
    setState(() {
      _customNote = '已加入清单：${reply.name}（${reply.kind}）';
    });
  }

  void _saveSettings() {
    final port = int.tryParse(_portController.text.trim()) ?? _config.port;
    final config = _config.copy()..port = port.clamp(1, 65535);
    config.setDomainsFromText(_domainsController.text);
    config.setExtraRulesFromText(_rulesController.text);
    _saveConfig(config);
    setState(() {
      _tool = _ToolPanel.none;
      _actionError = _running ? '已保存；重启服务后生效' : null;
    });
  }

  void _resetDefaultDomains() {
    setState(() {
      _domainsController.text = kDefaultRuleDomains.join('\n');
    });
  }

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
        // 上沿 1px 接缝盖在描边上：与搜索栏的分界线（与结果区同口径）。
        foregroundDecoration: BoxDecoration(
          border: Border(top: BorderSide(color: LunacColors.divider)),
        ),
        clipBehavior: Clip.antiAlias,
        child: _status == null ? _buildUnavailable() : _buildPanel(),
      ),
    );
  }

  Widget _buildUnavailable() {
    return Column(
      children: [
        _buildToolbar(),
        Expanded(
          child: Center(
            child: Text(
              '不可用（rust 桥接未加载）',
              style: TextStyle(color: LunacColors.textMuted, fontSize: 13),
            ),
          ),
        ),
      ],
    );
  }

  Widget _buildPanel() {
    return Column(
      children: [
        _buildToolbar(),
        Expanded(child: _buildStage()),
        _buildLogSplitter(),
        SizedBox(height: _logHeight, child: _buildLog()),
      ],
    );
  }

  Widget _buildToolbar() {
    final systemOn = _status?.system.enabled == true;
    return Padding(
      padding: const EdgeInsets.fromLTRB(10, 6, 8, 6),
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
            '代理',
            style: TextStyle(
              color: LunacColors.text,
              fontSize: 14,
              fontWeight: FontWeight.w600,
            ),
          ),
          const SizedBox(width: 8),
          Text(
            _running
                ? '自研引擎 · ${_engine?.port ?? 0} 端口'
                : '自研引擎 · 已停止',
            style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
          ),
          const Spacer(),
          Text(
            systemOn ? '系统代理：开' : '系统代理：关',
            style: TextStyle(
              color: systemOn ? LunacColors.textDim : LunacColors.textMuted,
              fontSize: 11,
            ),
          ),
          if (_status != null) ...[
            const SizedBox(width: 6),
            PopupMenuButton<String>(
              tooltip: '工具',
              icon: Icon(
                Icons.tune,
                size: 16,
                color: LunacColors.textMuted,
              ),
              padding: EdgeInsets.zero,
              splashRadius: 14,
              onSelected: (value) {
                switch (value) {
                  case 'import':
                    setState(() {
                      _importController.text = _config.linksText;
                      _tool = _ToolPanel.import;
                      _actionError = null;
                    });
                  case 'refresh':
                    _refreshSubscription();
                  case 'settings':
                    setState(() {
                      _portController.text = '${_config.port}';
                      _domainsController.text = _config.domainsText;
                      _rulesController.text = _config.rulesText;
                      _tool = _ToolPanel.settings;
                      _actionError = null;
                    });
                }
              },
              itemBuilder: (context) => const [
                PopupMenuItem(value: 'import', child: Text('服务器导入')),
                PopupMenuItem(value: 'refresh', child: Text('服务器订阅更新')),
                PopupMenuItem(value: 'settings', child: Text('设置')),
              ],
            ),
          ],
          // 独立窗：置顶 / 最小化 / 关闭 常驻右上角。
          if (widget.windowControls != null) ...[
            const SizedBox(width: 4),
            widget.windowControls!,
          ],
        ],
      ),
    );
  }

  Widget _buildStage() {
    switch (_tool) {
      case _ToolPanel.import:
        return _buildImportPanel();
      case _ToolPanel.settings:
        return _buildSettingsPanel();
      case _ToolPanel.none:
        break;
    }
    // v1 口径：抽屉展开时**铺满**中间区（右缘 60px 拉手收回），收起时露出大字开关
    // （左缘拉手展开）。这样在 640 档的搜索面板里也不会因为「宽不够」而没有入口。
    return Stack(
      children: [
        Positioned.fill(
          child: _drawerOpen ? _buildDrawer() : _buildCenter(),
        ),
        Positioned(
          left: _drawerOpen ? null : 0,
          right: _drawerOpen ? 0 : null,
          top: 0,
          bottom: 0,
          child: Center(child: _drawerHandle(open: !_drawerOpen)),
        ),
      ],
    );
  }

  /// 抽屉开合拉手（宽 22、高 60，竖排在抽屉右缘 / 中间区左缘）。
  Widget _drawerHandle({required bool open}) {
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: () => setState(() => _drawerOpen = open),
        child: SizedBox(
          width: 22,
          child: Center(
            child: Container(
              height: 60,
              decoration: BoxDecoration(
                color: LunacColors.background,
                border: Border.all(color: LunacColors.border),
                borderRadius: BorderRadius.horizontal(
                  right: Radius.circular(open ? 8 : 0),
                  left: Radius.circular(open ? 0 : 8),
                ),
              ),
              child: Icon(
                open ? Icons.chevron_right : Icons.chevron_left,
                size: 14,
                color: LunacColors.textMuted,
              ),
            ),
          ),
        ),
      ),
    );
  }

  Widget _buildDrawer() {
    final nodes = _nodes;
    return Container(
      decoration: BoxDecoration(
        color: LunacColors.background,
        border: Border(right: BorderSide(color: LunacColors.border)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(12, 8, 8, 6),
            child: Row(
              children: [
                Text(
                  '服务器',
                  style: TextStyle(
                    color: LunacColors.text,
                    fontSize: 12,
                    fontWeight: FontWeight.w600,
                  ),
                ),
                const SizedBox(width: 6),
                Text(
                  nodes.isEmpty ? '—' : '${nodes.length} 个',
                  style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
                ),
                const Spacer(),
                IconButton(
                  tooltip: '收起列表',
                  icon: const Icon(Icons.keyboard_double_arrow_left, size: 15),
                  color: LunacColors.textMuted,
                  visualDensity: VisualDensity.compact,
                  onPressed: () => setState(() => _drawerOpen = false),
                ),
              ],
            ),
          ),
          Container(height: 1, color: LunacColors.divider),
          Expanded(
            child: nodes.isEmpty
                ? Center(
                    child: Padding(
                      padding: const EdgeInsets.all(16),
                      child: Text(
                        '还没有服务器\n用右上角「工具 → 服务器导入」粘贴订阅或分享链接',
                        textAlign: TextAlign.center,
                        style: TextStyle(
                          color: LunacColors.textMuted,
                          fontSize: 11,
                          height: 1.5,
                        ),
                      ),
                    ),
                  )
                : ListView.builder(
                    padding: const EdgeInsets.symmetric(vertical: 4),
                    itemCount: nodes.length,
                    itemBuilder: (context, index) =>
                        _nodeRow(nodes[index], nodes[index].index == _active),
                  ),
          ),
        ],
      ),
    );
  }

  Widget _nodeRow(ProxyNode node, bool selected) {
    final delay = _delays[node.index];
    return InkWell(
      onTap: _busy || !_running ? null : () => _selectNode(node.index),
      child: Container(
        padding: const EdgeInsets.fromLTRB(12, 6, 8, 6),
        color: selected ? LunacColors.surfaceHover : Colors.transparent,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(
                  selected ? Icons.check_circle : Icons.circle_outlined,
                  size: 12,
                  color: selected ? LunacColors.accent : LunacColors.textMuted,
                ),
                const SizedBox(width: 6),
                Expanded(
                  child: Text(
                    node.name,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: node.implemented
                          ? LunacColors.textDim
                          : LunacColors.textMuted,
                      fontSize: 11.5,
                      fontWeight: selected ? FontWeight.w600 : FontWeight.normal,
                    ),
                  ),
                ),
                if (delay != null)
                  Text(
                    delay,
                    style: TextStyle(
                      color: LunacColors.textMuted,
                      fontSize: 10,
                    ),
                  ),
                const SizedBox(width: 6),
                InkWell(
                  onTap: () => _probeNode(node.index),
                  child: Text(
                    '测速',
                    style: TextStyle(
                      color: LunacColors.textMuted,
                      fontSize: 10,
                    ),
                  ),
                ),
              ],
            ),
            const SizedBox(height: 2),
            Text(
              '${node.kind}${node.transport.isEmpty ? '' : ' · ${node.transport}'}'
              '${node.implemented ? '' : '（未支持）'}',
              style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
            ),
            Text(
              '${node.endpoint}${node.source.isEmpty ? '' : '  ·  ${node.source}'}',
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildCenter() {
    return SingleChildScrollView(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
      child: Column(
        children: [
          _buildPowerButton(),
          const SizedBox(height: 10),
          Row(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              _rateChip(
                icon: Icons.arrow_downward,
                label: '下行',
                rate: _downRate,
              ),
              const SizedBox(width: 12),
              _rateChip(
                icon: Icons.arrow_upward,
                label: '上行',
                rate: _upRate,
              ),
            ],
          ),
          const SizedBox(height: 8),
          _buildExitRow(),
          const SizedBox(height: 12),
          _buildModeSwitch(),
          const SizedBox(height: 8),
          _buildTunRow(),
          if (_engine != null &&
              _engine!.skipped.isNotEmpty &&
              !_running) ...[
            const SizedBox(height: 10),
            _note('启动时被跳过的条目：'),
            for (final item in _engine!.skipped) _note(item),
          ],
          if (_actionError != null) ...[
            const SizedBox(height: 10),
            Text(
              _actionError!,
              textAlign: TextAlign.center,
              style: TextStyle(
                color: LunacColors.danger,
                fontSize: 11,
                height: 1.45,
              ),
            ),
          ],
        ],
      ),
    );
  }

  Widget _buildPowerButton() {
    final running = _running;
    final label = _busy ? '处理中…' : (running ? '关闭服务' : '开启服务');
    return Center(
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        child: GestureDetector(
          onTap: _busy ? null : _toggleService,
          child: AnimatedContainer(
            duration: const Duration(milliseconds: 180),
            width: 168,
            height: 168,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: running ? LunacColors.surfaceHover : LunacColors.background,
              border: Border.all(
                color: running ? LunacColors.accent : LunacColors.border,
                width: running ? 2 : 1,
              ),
              boxShadow: running
                  ? [
                      BoxShadow(
                        color: LunacColors.accent.withValues(alpha: 0.12),
                        blurRadius: 24,
                        spreadRadius: 2,
                      ),
                    ]
                  : null,
            ),
            child: Column(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                Icon(
                  running ? Icons.power_settings_new : Icons.power_settings_new,
                  size: 34,
                  color: running ? LunacColors.accent : LunacColors.textMuted,
                ),
                const SizedBox(height: 8),
                Text(
                  label,
                  style: TextStyle(
                    color: running ? LunacColors.text : LunacColors.textDim,
                    fontSize: 15,
                    fontWeight: FontWeight.w600,
                  ),
                ),
                const SizedBox(height: 4),
                Text(
                  running ? '运行中 · 点此停止' : '点此启动并接管系统代理',
                  style: TextStyle(
                    color: LunacColors.textMuted,
                    fontSize: 10,
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }

  Widget _rateChip({
    required IconData icon,
    required String label,
    required int rate,
  }) {
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        Icon(icon, size: 12, color: LunacColors.textMuted),
        const SizedBox(width: 3),
        Text(
          '$label ${_formatRate(rate)}',
          style: TextStyle(color: LunacColors.textDim, fontSize: 10.5),
        ),
      ],
    );
  }

  Widget _buildExitRow() {
    final text = _exitLoading
        ? '出口：探测中…'
        : _country.isNotEmpty
        ? '出口：${_countryName(_country)}（$_country）'
        : _countryError.isNotEmpty
        ? '出口：未探测到（$_countryError）'
        : '出口：未探测';
    return Row(
      mainAxisAlignment: MainAxisAlignment.center,
      children: [
        Text(
          text,
          style: TextStyle(color: LunacColors.textMuted, fontSize: 10.5),
        ),
        const SizedBox(width: 8),
        InkWell(
          onTap: _running && !_exitLoading ? _probeExit : null,
          child: Text(
            '探测',
            style: TextStyle(
              color: _running ? LunacColors.textDim : LunacColors.textMuted,
              fontSize: 10.5,
            ),
          ),
        ),
      ],
    );
  }

  Widget _buildModeSwitch() {
    return Row(
      mainAxisAlignment: MainAxisAlignment.center,
      children: [
        _modeChip('规则模式', 'rule'),
        const SizedBox(width: 6),
        _modeChip('全局代理', 'global'),
      ],
    );
  }

  Widget _modeChip(String label, String mode) {
    final active = _config.mode == mode;
    return InkWell(
      onTap: _busy ? null : () => _switchMode(mode),
      borderRadius: BorderRadius.circular(6),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 6),
        decoration: BoxDecoration(
          color: active ? LunacColors.surfaceHover : Colors.transparent,
          border: Border.all(
            color: active ? LunacColors.accent : LunacColors.border,
          ),
          borderRadius: BorderRadius.circular(6),
        ),
        child: Text(
          label,
          style: TextStyle(
            color: active ? LunacColors.text : LunacColors.textMuted,
            fontSize: 11,
          ),
        ),
      ),
    );
  }

  Widget _buildTunRow() {
    return Row(
      mainAxisAlignment: MainAxisAlignment.center,
      children: [
        Text(
          'TUN',
          style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
        ),
        const SizedBox(width: 6),
        Transform.scale(
          scale: 0.7,
          child: Switch(
            value: false,
            onChanged: _busy ? null : _tapTun,
            activeThumbColor: LunacColors.accent,
          ),
        ),
        Text(
          '（未实现）',
          style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
        ),
      ],
    );
  }

  Widget _note(String text) => Padding(
    padding: const EdgeInsets.only(bottom: 2),
    child: Text(
      text,
      textAlign: TextAlign.center,
      style: TextStyle(
        color: LunacColors.textMuted,
        fontSize: 10,
        height: 1.4,
      ),
    ),
  );

  Widget _buildImportPanel() {
    return SingleChildScrollView(
      padding: const EdgeInsets.fromLTRB(16, 10, 16, 16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _toolTitle('服务器导入', () => setState(() => _tool = _ToolPanel.none)),
          const SizedBox(height: 8),
          Text(
            '粘贴订阅（整段 base64 / Clash YAML）或分享链接，一行一条。'
            '与引擎共用同一个解析器（`parse_link`），不在这里另写一套协议编码。',
            style: TextStyle(
              color: LunacColors.textMuted,
              fontSize: 10.5,
              height: 1.5,
            ),
          ),
          const SizedBox(height: 8),
          TextField(
            controller: _importController,
            minLines: 4,
            maxLines: 8,
            style: TextStyle(color: LunacColors.text, fontSize: 11),
            decoration: _fieldDecoration(
              'ss://…  trojan://…  vless://…  vmess://…  anytls://…  hysteria2://…  tuic://…',
            ),
          ),
          const SizedBox(height: 10),
          Row(
            children: [
              _button('从剪贴板导入', _importFromClipboard),
              const SizedBox(width: 8),
              _button('保存到清单', _saveImport),
            ],
          ),
          const SizedBox(height: 14),
          Container(height: 1, color: LunacColors.divider),
          const SizedBox(height: 10),
          Text(
            '自定义服务器',
            style: TextStyle(
              color: LunacColors.text,
              fontSize: 12,
              fontWeight: FontWeight.w600,
            ),
          ),
          const SizedBox(height: 4),
          Text(
            '填字段 → 由引擎拼成分享链接，并用订阅同一个解析器回读校验，'
            '再追加到上面的清单（本版支持 ss / trojan / vless / vmess / anytls / hysteria2 / tuic）。',
            style: TextStyle(
              color: LunacColors.textMuted,
              fontSize: 10,
              height: 1.5,
            ),
          ),
          const SizedBox(height: 8),
          Row(
            children: [
              _kindDropdown(),
              const SizedBox(width: 8),
              Expanded(
                child: TextField(
                  controller: _customName,
                  style: TextStyle(color: LunacColors.text, fontSize: 11),
                  decoration: _fieldDecoration('名称（备注）'),
                ),
              ),
            ],
          ),
          const SizedBox(height: 6),
          Row(
            children: [
              Expanded(
                child: TextField(
                  controller: _customServer,
                  style: TextStyle(color: LunacColors.text, fontSize: 11),
                  decoration: _fieldDecoration('地址（域名 / IPv4 / IPv6）'),
                ),
              ),
              const SizedBox(width: 8),
              SizedBox(
                width: 90,
                child: TextField(
                  controller: _customPort,
                  keyboardType: TextInputType.number,
                  inputFormatters: [FilteringTextInputFormatter.digitsOnly],
                  style: TextStyle(color: LunacColors.text, fontSize: 11),
                  decoration: _fieldDecoration('端口'),
                ),
              ),
            ],
          ),
          const SizedBox(height: 6),
          if (_customKind == 'tuic') ...[
            TextField(
              controller: _customUuid,
              style: TextStyle(color: LunacColors.text, fontSize: 11),
              decoration: _fieldDecoration('UUID'),
            ),
            const SizedBox(height: 6),
          ],
          Row(
            children: [
              Expanded(
                child: TextField(
                  controller: _customSecret,
                  obscureText: false,
                  style: TextStyle(color: LunacColors.text, fontSize: 11),
                  decoration: _fieldDecoration(
                    _customKind == 'vless' || _customKind == 'vmess'
                        ? 'UUID'
                        : '密码',
                  ),
                ),
              ),
              if (_customKind == 'ss') ...[
                const SizedBox(width: 8),
                SizedBox(
                  width: 170,
                  child: TextField(
                    controller: _customMethod,
                    style: TextStyle(color: LunacColors.text, fontSize: 11),
                    decoration: _fieldDecoration('加密方式'),
                  ),
                ),
              ],
            ],
          ),
          if (_customKind == 'trojan' ||
              _customKind == 'vless' ||
              _customKind == 'vmess') ...[
            const SizedBox(height: 4),
            Row(
              children: [
                Transform.scale(
                  scale: 0.75,
                  child: Switch(
                    value: _customMux,
                    onChanged: (value) => setState(() => _customMux = value),
                    activeThumbColor: LunacColors.accent,
                  ),
                ),
                const SizedBox(width: 6),
                Expanded(
                  child: Text(
                    'mux.cool 多路复用（Xray 服务端；Clash 的 smux 是另一套协议）',
                    style: TextStyle(
                      color: LunacColors.textMuted,
                      fontSize: 10,
                    ),
                  ),
                ),
              ],
            ),
          ],
          const SizedBox(height: 8),
          Row(
            children: [
              _button('生成并加入清单', _addCustomServer),
              const SizedBox(width: 8),
              Expanded(
                child: Text(
                  _customNote,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: _customNote.startsWith('已加入')
                        ? LunacColors.textDim
                        : LunacColors.danger,
                    fontSize: 10,
                    height: 1.4,
                  ),
                ),
              ),
            ],
          ),
          if (_actionError != null) ...[
            const SizedBox(height: 10),
            Text(
              _actionError!,
              style: TextStyle(
                color: LunacColors.danger,
                fontSize: 11,
                height: 1.45,
              ),
            ),
          ],
        ],
      ),
    );
  }

  Widget _buildSettingsPanel() {
    return SingleChildScrollView(
      padding: const EdgeInsets.fromLTRB(16, 10, 16, 16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _toolTitle('设置', () => setState(() => _tool = _ToolPanel.none)),
          const SizedBox(height: 12),
          Row(
            children: [
              Text(
                '规则配置文件',
                style: TextStyle(color: LunacColors.text, fontSize: 12),
              ),
              const Spacer(),
              Text(
                _config.ruleMode == 'whitelist' ? '白名单' : '黑名单',
                style: TextStyle(color: LunacColors.textMuted, fontSize: 10.5),
              ),
              const SizedBox(width: 6),
              Transform.scale(
                scale: 0.75,
                child: Switch(
                  value: _config.ruleMode == 'whitelist',
                  onChanged: (value) => setState(() {
                    final config = _config.copy()
                      ..ruleMode = value ? 'whitelist' : 'blacklist';
                    _saveConfig(config);
                  }),
                  activeThumbColor: LunacColors.accent,
                ),
              ),
            ],
          ),
          Text(
            '黑名单 = 名单内直连、名单外走代理；白名单 = 名单内走代理、名单外直连。'
            '这一层排在订阅规则之前（先匹配先赢），与引擎的 `rules::domain_rules` 同一口径。',
            style: TextStyle(
              color: LunacColors.textMuted,
              fontSize: 10,
              height: 1.5,
            ),
          ),
          const SizedBox(height: 8),
          TextField(
            controller: _domainsController,
            minLines: 6,
            maxLines: 10,
            style: TextStyle(
              color: LunacColors.textDim,
              fontSize: 10.5,
              height: 1.4,
            ),
            decoration: _fieldDecoration('一行一个域名；# 或 // 开头的行忽略'),
          ),
          const SizedBox(height: 12),
          Text(
            '自定义规则',
            style: TextStyle(color: LunacColors.text, fontSize: 12),
          ),
          Text(
            '一行一条，Clash 语法：DOMAIN / DOMAIN-SUFFIX / DOMAIN-KEYWORD / IP-CIDR / '
            'IP-CIDR6 / PROCESS-NAME / GEOIP / MATCH（例如 `GEOIP,CN,DIRECT`）。'
            '这一层排在名单层之后、订阅规则之前；`RULE-SET` 暂不支持，会在启动结果的跳过里写明。',
            style: TextStyle(
              color: LunacColors.textMuted,
              fontSize: 10,
              height: 1.5,
            ),
          ),
          const SizedBox(height: 8),
          TextField(
            controller: _rulesController,
            minLines: 3,
            maxLines: 6,
            style: TextStyle(
              color: LunacColors.textDim,
              fontSize: 10.5,
              height: 1.4,
            ),
            decoration: _fieldDecoration('一行一条；# 或 // 开头的行忽略'),
          ),
          const SizedBox(height: 6),
          Row(
            children: [
              _button('恢复默认清单', _resetDefaultDomains),
              const SizedBox(width: 8),
              SizedBox(
                width: 96,
                child: TextField(
                  controller: _portController,
                  keyboardType: TextInputType.number,
                  inputFormatters: [FilteringTextInputFormatter.digitsOnly],
                  style: TextStyle(color: LunacColors.text, fontSize: 11),
                  decoration: _fieldDecoration('端口'),
                ),
              ),
            ],
          ),
          const SizedBox(height: 10),
          Row(
            children: [
              _button('保存', _saveSettings),
              const SizedBox(width: 8),
              Text(
                _running ? '运行中：保存后需重启服务' : '停止状态下保存后立即生效',
                style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
              ),
            ],
          ),
          if (_actionError != null) ...[
            const SizedBox(height: 10),
            Text(
              _actionError!,
              style: TextStyle(
                color: LunacColors.danger,
                fontSize: 11,
                height: 1.45,
              ),
            ),
          ],
        ],
      ),
    );
  }

  Widget _toolTitle(String title, VoidCallback onBack) {
    return Row(
      children: [
        InkWell(
          onTap: onBack,
          child: Row(
            children: [
              Icon(Icons.arrow_back, size: 15, color: LunacColors.textMuted),
              const SizedBox(width: 4),
              Text(
                '返回',
                style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
              ),
            ],
          ),
        ),
        const SizedBox(width: 12),
        Text(
          title,
          style: TextStyle(
            color: LunacColors.text,
            fontSize: 12,
            fontWeight: FontWeight.w600,
          ),
        ),
      ],
    );
  }

  /// 自定义服务器的协议下拉（ss / trojan / vless / vmess / anytls / hysteria2 / tuic）。
  Widget _kindDropdown() {
    return Container(
      height: 34,
      padding: const EdgeInsets.symmetric(horizontal: 8),
      decoration: BoxDecoration(
        color: LunacColors.background,
        border: Border.all(color: LunacColors.border),
        borderRadius: BorderRadius.circular(6),
      ),
      child: DropdownButtonHideUnderline(
        child: DropdownButton<String>(
          value: _customKind,
          dropdownColor: LunacColors.surface,
          style: TextStyle(color: LunacColors.text, fontSize: 11),
          icon: Icon(
            Icons.arrow_drop_down,
            size: 16,
            color: LunacColors.textMuted,
          ),
          items: const [
            DropdownMenuItem(value: 'ss', child: Text('ss')),
            DropdownMenuItem(value: 'trojan', child: Text('trojan')),
            DropdownMenuItem(value: 'vless', child: Text('vless')),
            DropdownMenuItem(value: 'vmess', child: Text('vmess')),
            DropdownMenuItem(value: 'anytls', child: Text('anytls')),
            DropdownMenuItem(value: 'hysteria2', child: Text('hysteria2')),
            DropdownMenuItem(value: 'tuic', child: Text('tuic')),
          ],
          onChanged: (value) {
            if (value == null) {
              return;
            }
            setState(() {
              _customKind = value;
              _customNote = '';
            });
          },
        ),
      ),
    );
  }

  Widget _button(String label, VoidCallback? onPressed) {
    return SizedBox(
      height: 30,
      child: OutlinedButton(
        onPressed: _busy ? null : onPressed,
        style: OutlinedButton.styleFrom(
          padding: const EdgeInsets.symmetric(horizontal: 12),
          side: BorderSide(color: LunacColors.border),
          foregroundColor: LunacColors.text,
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(6),
          ),
        ),
        child: Text(label, style: const TextStyle(fontSize: 11)),
      ),
    );
  }

  InputDecoration _fieldDecoration(String hint) {
    return InputDecoration(
      isDense: true,
      hintText: hint,
      hintStyle: TextStyle(color: LunacColors.textMuted, fontSize: 11),
      filled: true,
      fillColor: LunacColors.background,
      contentPadding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
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
    );
  }

  /// 日志区上边界：上下拖拽改高度，松手写回本地存储（v1 同口径）。
  Widget _buildLogSplitter() {
    return MouseRegion(
      cursor: SystemMouseCursors.resizeUpDown,
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onVerticalDragUpdate: (details) {
          setState(() {
            _logHeight = (_logHeight - details.delta.dy).clamp(
              _logMinHeight,
              _logMaxHeight,
            );
          });
        },
        onVerticalDragEnd: (_) {
          writePluginKv(
            id: _kvId,
            key: _kvLogHeightKey,
            value: _logHeight,
          );
        },
        child: Container(
          height: 6,
          alignment: Alignment.center,
          child: Container(height: 1, color: LunacColors.border),
        ),
      ),
    );
  }

  Widget _buildLog() {
    return Container(
      color: LunacColors.terminal,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(12, 5, 8, 5),
            child: Row(
              children: [
                Text(
                  '请求日志',
                  style: TextStyle(
                    color: LunacColors.textDim,
                    fontSize: 11,
                    fontWeight: FontWeight.w600,
                  ),
                ),
                const SizedBox(width: 6),
                Text(
                  _running ? '每 2s 刷新 · 最近 ${_log.length} 条' : '服务停止时不产生日志',
                  style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
                ),
                const Spacer(),
                Text(
                  '目标域名:端口 → 来源 / 节点 / 直连 / 拒绝',
                  style: TextStyle(color: LunacColors.textMuted, fontSize: 9.5),
                ),
              ],
            ),
          ),
          Container(height: 1, color: LunacColors.border),
          Expanded(
            child: _log.isEmpty
                ? Center(
                    child: Text(
                      '暂无请求',
                      style: TextStyle(
                        color: LunacColors.textMuted,
                        fontSize: 11,
                      ),
                    ),
                  )
                : ListView.builder(
                    controller: _logScroll,
                    padding: const EdgeInsets.symmetric(
                      horizontal: 12,
                      vertical: 4,
                    ),
                    itemCount: _log.length,
                    itemBuilder: (context, index) {
                      final entry = _log[index];
                      return Padding(
                        padding: const EdgeInsets.symmetric(vertical: 1),
                        child: Text(
                          entry.line,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                            color: _routeColor(entry.route),
                            fontSize: 10.5,
                            fontFeatures: const [FontFeature.tabularFigures()],
                          ),
                        ),
                      );
                    },
                  ),
          ),
        ],
      ),
    );
  }

  Color _routeColor(String route) {
    switch (route) {
      case 'node':
        return LunacColors.accent;
      case 'rejected':
        return LunacColors.danger;
      default:
        return LunacColors.textMuted;
    }
  }

  static String _formatRate(int bytesPerSecond) {
    if (bytesPerSecond <= 0) {
      return '0 B/s';
    }
    if (bytesPerSecond < 1024) {
      return '$bytesPerSecond B/s';
    }
    final kb = bytesPerSecond / 1024.0;
    if (kb < 1024) {
      return '${kb.toStringAsFixed(kb < 10 ? 1 : 0)} KB/s';
    }
    final mb = kb / 1024.0;
    return '${mb.toStringAsFixed(mb < 10 ? 1 : 0)} MB/s';
  }

  static String _countryName(String code) {
    const names = {
      'CN': '中国',
      'HK': '香港',
      'TW': '台湾',
      'JP': '日本',
      'KR': '韩国',
      'SG': '新加坡',
      'US': '美国',
      'GB': '英国',
      'DE': '德国',
      'FR': '法国',
      'NL': '荷兰',
      'CA': '加拿大',
      'AU': '澳大利亚',
      'RU': '俄罗斯',
      'IN': '印度',
      'TR': '土耳其',
    };
    return names[code.toUpperCase()] ?? code.toUpperCase();
  }
}
