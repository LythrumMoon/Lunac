// 插件运行时面板（v1 `plugin_sidecar` 的宿主侧界面；v2 用 Flutter 画，不加载第三方 WebView）。
//
// 它解决的问题：v1 的插件是 TS + WebView；v2 不加载第三方 WebView（唯一例外是桌宠）。
// 但「AI 开发的插件可以用任何框架 / 语言」不能丢 —— 插件在清单里声明 `sidecar`，
// 把自己的可执行文件（Python / Node / Go / Rust…）交给宿主起进程，本面板就是
// 宿主侧的控制台：清单 / 信任卡 / 起停 / stdio 调用 / 本机端口代请求 / 事件流。
//
// 口径（与 v1、`docs/forbidden.md` 一致）：
//   · **不是沙箱**：sidecar 是完整本机进程；信任卡是「如实告知 + 用户裁决」，不是隔离；
//   · 缺省只放行插件**自己上报的端口**；串其他本机应用必须在清单里声明并由用户信任；
//   · 没有可执行文件 / 没有 sidecar 段时如实说明，**不放点了没反应的按钮**。
import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';

import '../platform/plugin_runtime.dart';
import '../theme.dart';

/// 面板所属的 owner（关窗时按它收进程）。
const String kPluginRuntimeOwner = 'plugin-console';

class PluginConsolePanel extends StatefulWidget {
  const PluginConsolePanel({
    super.key,
    required this.onClose,
    required this.pluginId,
    this.bridge = const NativePluginRuntimeBridge(),
    this.refreshInterval = const Duration(seconds: 1),
    this.windowControls,
  });

  final VoidCallback onClose;

  /// 独立窗的「置顶 / 最小化 / 关闭」（由 `plugin_window_main` 注入；常驻右上角）。
  final Widget? windowControls;

  /// 要操作的插件 id（由设置页「运行时」按钮带进窗口）。
  final String pluginId;

  /// 测试注入用；null 之外的默认实现走 C ABI。
  final PluginRuntimeBridge bridge;

  /// 状态 / 事件轮询间隔；`null` = 不轮询（单测用，避免 `pumpAndSettle` 永不收敛）。
  final Duration? refreshInterval;

  @override
  State<PluginConsolePanel> createState() => _PluginConsolePanelState();
}

class _PluginConsolePanelState extends State<PluginConsolePanel> {
  final TextEditingController _method = TextEditingController(text: 'ping');
  final TextEditingController _params = TextEditingController();
  final TextEditingController _httpMethod = TextEditingController(text: 'GET');
  final TextEditingController _httpPath = TextEditingController(text: '/health');
  final TextEditingController _httpBody = TextEditingController();
  final TextEditingController _httpPort = TextEditingController();

  PluginSidecarStatus? _status;
  PluginSidecarStart? _pendingTrust;
  PluginRuntimeCall? _lastCall;
  PluginRuntimeHttp? _lastHttp;
  final List<Map<String, Object?>> _events = [];
  int _cursor = 0;
  bool _busy = false;
  String? _message;
  bool _messageIsError = false;
  Timer? _timer;
  bool _disposed = false;

  @override
  void initState() {
    super.initState();
    _load();
    final interval = widget.refreshInterval;
    if (interval != null) {
      _timer = Timer.periodic(interval, (_) => _poll());
    }
  }

  @override
  void dispose() {
    _disposed = true;
    _timer?.cancel();
    // 面板关掉 = 它起的 sidecar 一并收掉（v1 契约 ⑥）。
    widget.bridge.stopOwner(kPluginRuntimeOwner);
    _method.dispose();
    _params.dispose();
    _httpMethod.dispose();
    _httpPath.dispose();
    _httpBody.dispose();
    _httpPort.dispose();
    super.dispose();
  }

  void _load() {
    final status = widget.bridge.readStatus(widget.pluginId);
    if (_disposed) {
      return;
    }
    setState(() {
      _status = status;
      _cursor = status.cursor;
    });
  }

  void _poll() {
    if (_disposed) {
      return;
    }
    final status = widget.bridge.readStatus(widget.pluginId);
    final events = widget.bridge.poll(widget.pluginId, _cursor);
    if (_disposed) {
      return;
    }
    setState(() {
      _status = status;
      _cursor = events.cursor;
      _events.addAll(events.events);
      if (_events.length > 100) {
        _events.removeRange(0, _events.length - 100);
      }
      if (events.exitReason.isNotEmpty && !status.running) {
        _status = status;
      }
    });
  }

  void _note(String text, {bool error = false}) {
    setState(() {
      _message = text;
      _messageIsError = error;
    });
  }

  Future<void> _start() async {
    if (_busy) {
      return;
    }
    setState(() {
      _busy = true;
      _message = null;
      _messageIsError = false;
    });
    final result = await widget.bridge.start(widget.pluginId, kPluginRuntimeOwner);
    if (_disposed) {
      return;
    }
    setState(() {
      _busy = false;
      _pendingTrust = result.needsTrust ? result : null;
      if (result.needsTrust) {
        _note('这个插件要起自己的本机进程，请先确认下面的内容');
      } else if (result.started) {
        _note(
          result.port == null
              ? '已启动（stdio 通道）'
              : '已启动（http 通道，端口 ${result.port}）',
        );
      } else {
        _note(result.error.isEmpty ? '启动失败' : result.error, error: true);
      }
    });
    _load();
  }

  Future<void> _stop() async {
    if (_busy) {
      return;
    }
    setState(() => _busy = true);
    final reply = await widget.bridge.stop(widget.pluginId);
    if (_disposed) {
      return;
    }
    setState(() {
      _busy = false;
      _pendingTrust = null;
    });
    _note(reply.ok ? '已停止' : reply.error, error: !reply.ok);
    _load();
  }

  Future<void> _trust({required bool remember}) async {
    setState(() => _busy = true);
    final reply = await widget.bridge.trust(
      widget.pluginId,
      remember: remember,
    );
    if (_disposed) {
      return;
    }
    setState(() => _busy = false);
    if (!reply.ok) {
      _note(reply.error, error: true);
      return;
    }
    _note(remember ? '已信任并记住（写 config/plugin-trusted.json）' : '已信任（仅本次）');
    setState(() => _pendingTrust = null);
    await _start();
  }

  Future<void> _deny() async {
    setState(() => _busy = true);
    final reply = await widget.bridge.deny(widget.pluginId);
    if (_disposed) {
      return;
    }
    setState(() {
      _busy = false;
      _pendingTrust = null;
    });
    _note(reply.ok ? '已拒绝（本会话内不再询问）' : reply.error, error: !reply.ok);
    _load();
  }

  Future<void> _send() async {
    final method = _method.text.trim();
    if (method.isEmpty) {
      _note('先写一个 method（例如 ping / status）', error: true);
      return;
    }
    final params = _params.text.trim();
    if (params.isNotEmpty) {
      try {
        // 只做语法检查；真正的 JSON 由 Rust 侧再解析一次。
        jsonDecode(params);
      } catch (_) {
        _note('params 不是合法 JSON', error: true);
        return;
      }
    }
    setState(() => _busy = true);
    final call = await widget.bridge.request(widget.pluginId, method, params);
    if (_disposed) {
      return;
    }
    setState(() {
      _busy = false;
      _lastCall = call;
    });
    _poll();
  }

  Future<void> _sendHttp() async {
    final method = _httpMethod.text.trim();
    final path = _httpPath.text.trim();
    if (method.isEmpty || !path.startsWith('/')) {
      _note('HTTP 方法与路径都要填，路径必须以 / 开头', error: true);
      return;
    }
    final port = int.tryParse(_httpPort.text.trim()) ?? 0;
    setState(() => _busy = true);
    final reply = await widget.bridge.http(
      widget.pluginId,
      method,
      path,
      _httpBody.text,
      port,
    );
    if (_disposed) {
      return;
    }
    setState(() {
      _busy = false;
      _lastHttp = reply;
    });
  }

  @override
  Widget build(BuildContext context) {
    final status = _status;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _buildHeader(status),
        if (_message != null)
          Container(
            width: double.infinity,
            padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 7),
            color: (_messageIsError ? LunacColors.danger : LunacColors.accent)
                .withValues(alpha: 0.08),
            child: Text(
              _message!,
              style: TextStyle(
                color: _messageIsError
                    ? LunacColors.danger
                    : LunacColors.textDim,
                fontSize: 11,
                height: 1.45,
              ),
            ),
          ),
        Expanded(
          child: ListView(
            padding: const EdgeInsets.fromLTRB(14, 4, 14, 16),
            children: [
              _hint(
                '插件运行在自己的进程里 —— Python / Node / Go / Rust 都行；界面由 Lunac 画。'
                '这不是沙箱：信任卡是如实告知与用户裁决，不是隔离。',
              ),
              if (status == null)
                _section('状态', [_muted('读取中…')])
              else if (!status.ok)
                _section('状态', [_errorLine(status.error)])
              else if (!status.hasSidecar)
                _section('状态', [
                  _muted('这个插件没有在清单里声明 sidecar，没有可起的外部进程。'),
                  _muted('入口 ${status.pluginId}/index.js 是 v1 的 WebView 插件形态，'
                      'v2 不加载第三方 WebView（桌宠除外）。'),
                ])
              else ...[
                _buildSpec(status),
                if (!status.trusted && !status.denied)
                  _buildTrustCard(status, _pendingTrust),
                if (status.denied)
                  _section('信任', [
                    _muted('本会话已拒绝该 sidecar；重启 Lunac 后会重新询问。'),
                  ]),
                _buildCall(status),
                if (status.spec?.usesHttp ?? false) _buildHttp(status),
                _buildEvents(),
              ],
            ],
          ),
        ),
      ],
    );
  }

  Widget _buildHeader(PluginSidecarStatus? status) {
    final running = status?.running ?? false;
    final trustLabel = status == null
        ? ''
        : status.trusted
        ? '已信任'
        : status.denied
        ? '已拒绝'
        : '待信任';
    return Container(
      padding: const EdgeInsets.fromLTRB(14, 10, 10, 8),
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: LunacColors.border)),
      ),
      child: Row(
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    Flexible(
                      child: Text(
                        widget.pluginId,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          color: LunacColors.text,
                          fontSize: 13,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                    ),
                    const SizedBox(width: 8),
                    _chip(
                      running ? '运行中' : '未运行',
                      color: running ? LunacColors.accent : LunacColors.textMuted,
                    ),
                    const SizedBox(width: 6),
                    if (trustLabel.isNotEmpty)
                      _chip(
                        trustLabel,
                        color: status?.trusted == true
                            ? LunacColors.textDim
                            : LunacColors.warning,
                      ),
                  ],
                ),
                const SizedBox(height: 2),
                Text(
                  '插件运行时${status?.port == null ? '' : ' · 端口 ${status!.port}'}',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(color: LunacColors.textMuted, fontSize: 10.5),
                ),
              ],
            ),
          ),
          const SizedBox(width: 8),
          // 没有 sidecar 的插件不给起停按钮（不放点了没反应的按钮）。
          if (status?.hasSidecar == true)
            if (running)
              _button('停止', onPressed: _busy ? null : _stop, danger: true)
            else
              _button('启动', onPressed: _busy ? null : _start),
          // 独立窗：置顶 / 最小化 / 关闭 常驻右上角（用户 2026-11-20 口径）。
          if (widget.windowControls != null) ...[
            const SizedBox(width: 6),
            widget.windowControls!,
          ],
        ],
      ),
    );
  }

  Widget _buildSpec(PluginSidecarStatus status) {
    final spec = status.spec;
    if (spec == null) {
      return _section('清单', [_muted('清单里的 sidecar 段读不出来。')]);
    }
    return _section('清单', [
      _kv('可执行文件', spec.command),
      if (spec.args.isNotEmpty) _kv('参数', spec.args.join(' ')),
      _kv(
        '通道',
        spec.transport.isEmpty
            ? 'stdio（缺省）'
            : '${spec.transport}${spec.port == 0 ? '' : ' · 端口 ${spec.port}'}',
      ),
      _kv('就绪超时', '${spec.readyTimeoutSec}s'),
      if (spec.restart.isNotEmpty) _kv('崩溃重启', spec.restart),
      if (spec.sha256.isNotEmpty)
        _kv('校验和', spec.sha256, mono: true),
      _kv(
        '授权端口',
        spec.allowLocalAny
            ? '任意本机端口（allowLocalAny）'
            : spec.allowLocalPorts.isEmpty
            ? '只放行插件自己上报的端口'
            : spec.allowLocalPorts.join(', '),
      ),
      if (spec.memoryLimitMiB > 0 || spec.processLimit > 0)
        _kv(
          '资源闸',
          [
            if (spec.memoryLimitMiB > 0) '内存 ${spec.memoryLimitMiB} MiB',
            if (spec.processLimit > 0) '进程 ${spec.processLimit}',
          ].join(' · '),
        ),
      if (status.exitReason.isNotEmpty && !status.running)
        _kv('上次退出', status.exitReason, color: LunacColors.warning),
    ]);
  }

  Widget _buildTrustCard(
    PluginSidecarStatus status,
    PluginSidecarStart? pending,
  ) {
    final command = pending?.command.isNotEmpty == true
        ? pending!.command
        : (status.spec?.command ?? '');
    final args = pending?.args.isNotEmpty == true
        ? pending!.args
        : (status.spec?.args ?? const <String>[]);
    final sha = pending?.sha256.isNotEmpty == true
        ? pending!.sha256
        : (status.spec?.sha256 ?? '');
    final ports = pending?.allowLocalPorts.isNotEmpty == true
        ? pending!.allowLocalPorts
        : (status.spec?.allowLocalPorts ?? const <int>[]);
    final allowAny = pending?.allowLocalAny ?? status.spec?.allowLocalAny ?? false;
    return Container(
      margin: const EdgeInsets.only(top: 10),
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: LunacColors.surface,
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: LunacColors.warning.withValues(alpha: 0.55)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Icon(Icons.gpp_maybe_outlined, size: 15, color: LunacColors.warning),
              const SizedBox(width: 6),
              Text(
                '信任卡',
                style: TextStyle(
                  color: LunacColors.warning,
                  fontSize: 12,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ],
          ),
          const SizedBox(height: 8),
          Text(
            '这个插件要在你的电脑上起下面这个进程。装谁 = 信任谁；命令或校验和一变，'
            '旧信任会立即失效并重新询问。',
            style: TextStyle(color: LunacColors.textDim, fontSize: 11, height: 1.5),
          ),
          const SizedBox(height: 8),
          _kv('命令', command, mono: true),
          if (args.isNotEmpty) _kv('参数', args.join(' '), mono: true),
          if (sha.isNotEmpty) _kv('sha256', sha, mono: true),
          if (allowAny)
            _kv('授权', '任意本机端口（会显著放宽）', color: LunacColors.warning)
          else if (ports.isNotEmpty)
            _kv('授权端口', ports.join(', '))
          else
            _kv('授权端口', '只放行它自己上报的端口'),
          const SizedBox(height: 10),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: [
              _button('允许并记住', onPressed: _busy ? null : () => _trust(remember: true)),
              _button(
                '仅本次允许',
                onPressed: _busy ? null : () => _trust(remember: false),
              ),
              _button('拒绝', onPressed: _busy ? null : _deny, danger: true),
            ],
          ),
        ],
      ),
    );
  }

  Widget _buildCall(PluginSidecarStatus status) {
    return _section('stdio 调用', [
      Row(
        children: [
          SizedBox(
            width: 180,
            child: _field(_method, 'method（如 ping）'),
          ),
          const SizedBox(width: 8),
          Expanded(child: _field(_params, 'params（JSON，可空）', mono: true)),
          const SizedBox(width: 8),
          _button('发送', onPressed: _busy ? null : _send),
        ],
      ),
      if (_lastCall != null)
        Container(
          margin: const EdgeInsets.only(top: 8),
          padding: const EdgeInsets.all(10),
          constraints: const BoxConstraints(maxHeight: 220),
          decoration: BoxDecoration(
            color: LunacColors.terminal,
            borderRadius: BorderRadius.circular(6),
          ),
          child: SingleChildScrollView(
            child: SelectableText(
              _lastCall!.resultText,
              style: TextStyle(
                color: _lastCall!.ok ? LunacColors.textDim : LunacColors.danger,
                fontSize: 11,
                height: 1.45,
                fontFamily: 'Consolas',
              ),
            ),
          ),
        ),
      if (!status.running)
        _muted('进程没起来时点「发送」会得到「sidecar 未启动」—— 先点右上角「启动」。'),
    ]);
  }

  Widget _buildHttp(PluginSidecarStatus status) {
    return _section('本机端口代请求', [
      _muted('由 Lunac 去连 127.0.0.1；插件代码自己从不碰端口（CSP 不放宽）。'),
      const SizedBox(height: 6),
      Row(
        children: [
          SizedBox(width: 88, child: _field(_httpMethod, '方法')),
          const SizedBox(width: 8),
          Expanded(child: _field(_httpPath, '路径（以 / 开头）', mono: true)),
          const SizedBox(width: 8),
          SizedBox(width: 100, child: _field(_httpPort, '端口（0=自带）')),
          const SizedBox(width: 8),
          _button('代请求', onPressed: _busy ? null : _sendHttp),
        ],
      ),
      const SizedBox(height: 6),
      _field(_httpBody, 'body（可空）', mono: true, maxLines: 2),
      if (_lastHttp != null)
        Container(
          margin: const EdgeInsets.only(top: 8),
          padding: const EdgeInsets.all(10),
          constraints: const BoxConstraints(maxHeight: 220),
          decoration: BoxDecoration(
            color: LunacColors.terminal,
            borderRadius: BorderRadius.circular(6),
          ),
          child: SingleChildScrollView(
            child: SelectableText(
              _lastHttp!.ok
                  ? '${_lastHttp!.status}\n${_lastHttp!.body}'
                  : _lastHttp!.error,
              style: TextStyle(
                color: _lastHttp!.ok ? LunacColors.textDim : LunacColors.danger,
                fontSize: 11,
                height: 1.45,
                fontFamily: 'Consolas',
              ),
            ),
          ),
        ),
    ]);
  }

  Widget _buildEvents() {
    return _section('事件流', [
      if (_events.isEmpty)
        _muted('还没有主动推事件。sidecar 的 `{"method":"…"}` 行会出现在这里。')
      else
        Container(
          constraints: const BoxConstraints(maxHeight: 240),
          decoration: BoxDecoration(
            color: LunacColors.terminal,
            borderRadius: BorderRadius.circular(6),
          ),
          child: ListView.builder(
            padding: const EdgeInsets.all(8),
            itemCount: _events.length,
            itemBuilder: (context, index) {
              final event = _events[index];
              final seq = event['seq'];
              final payload = event['payload'];
              return Padding(
                padding: const EdgeInsets.only(bottom: 4),
                child: SelectableText(
                  '#$seq ${_compact(payload)}',
                  style: TextStyle(
                    color: LunacColors.textDim,
                    fontSize: 11,
                    height: 1.4,
                    fontFamily: 'Consolas',
                  ),
                ),
              );
            },
          ),
        ),
    ]);
  }

  String _compact(Object? payload) {
    final text = payload.toString();
    return text.length <= 400 ? text : '${text.substring(0, 400)}…';
  }

  Widget _section(String title, List<Widget> children) {
    return Container(
      margin: const EdgeInsets.only(top: 12),
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: LunacColors.surface,
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: LunacColors.border),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            title,
            style: TextStyle(
              color: LunacColors.text,
              fontSize: 12,
              fontWeight: FontWeight.w600,
            ),
          ),
          const SizedBox(height: 8),
          ...children,
        ],
      ),
    );
  }

  Widget _kv(String key, String value, {bool mono = false, Color? color}) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 4),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            width: 76,
            child: Text(
              key,
              style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
            ),
          ),
          Expanded(
            child: SelectableText(
              value,
              style: TextStyle(
                color: color ?? LunacColors.textDim,
                fontSize: 11,
                height: 1.45,
                fontFamily: mono ? 'Consolas' : null,
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _hint(String text) {
    return Padding(
      padding: const EdgeInsets.only(top: 10, bottom: 2),
      child: Text(
        text,
        style: TextStyle(color: LunacColors.textMuted, fontSize: 10.5, height: 1.5),
      ),
    );
  }

  Widget _muted(String text) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 4),
      child: Text(
        text,
        style: TextStyle(color: LunacColors.textMuted, fontSize: 11, height: 1.45),
      ),
    );
  }

  Widget _errorLine(String text) {
    return Text(
      text,
      style: TextStyle(color: LunacColors.danger, fontSize: 11, height: 1.45),
    );
  }

  Widget _chip(String text, {required Color color}) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 1),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(4),
        border: Border.all(color: color.withValues(alpha: 0.45)),
      ),
      child: Text(text, style: TextStyle(color: color, fontSize: 9.5)),
    );
  }

  Widget _button(
    String label, {
    VoidCallback? onPressed,
    bool danger = false,
  }) {
    final color = danger ? LunacColors.danger : LunacColors.accent;
    return TextButton(
      onPressed: onPressed,
      style: TextButton.styleFrom(
        minimumSize: const Size(0, 26),
        padding: const EdgeInsets.symmetric(horizontal: 10),
        foregroundColor: color,
        side: BorderSide(color: color.withValues(alpha: onPressed == null ? 0.2 : 0.5)),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(6)),
        textStyle: const TextStyle(fontSize: 11),
      ),
      child: Text(label),
    );
  }

  Widget _field(
    TextEditingController controller,
    String hint, {
    bool mono = false,
    int maxLines = 1,
  }) {
    return TextField(
      controller: controller,
      maxLines: maxLines,
      style: TextStyle(
        color: LunacColors.text,
        fontSize: 11.5,
        fontFamily: mono ? 'Consolas' : null,
      ),
      decoration: InputDecoration(
        hintText: hint,
        hintStyle: TextStyle(color: LunacColors.textMuted, fontSize: 10.5),
        isDense: true,
        contentPadding: const EdgeInsets.symmetric(horizontal: 8, vertical: 8),
        border: OutlineInputBorder(
          borderRadius: BorderRadius.circular(6),
          borderSide: BorderSide(color: LunacColors.border),
        ),
        enabledBorder: OutlineInputBorder(
          borderRadius: BorderRadius.circular(6),
          borderSide: BorderSide(color: LunacColors.border),
        ),
      ),
    );
  }
}
