// 预览区终端标签（用户 2026-11-15：预览区做成「小 VSCode」，要一个终端）。
//
// 数据层在 `platform/terminal.dart`（Rust 的 `terminal.rs`）：优先 ConPTY 真伪终端，
// 系统不支持时降级成 `cmd.exe /Q /K` 的管道模式。本文件只做两件事：
//   1. 每 120 ms 把 Rust 缓冲里的输出取回来（C ABI 不能反向回调进 Flutter）；
//   2. 把键盘输入写回去（ConPTY 逐键；管道档在底部输入行里攒到 Enter 整行发）。
//
// 画面用 `parseAnsi` 回放（与对话工具卡的 xterm 等价物同一份实现）：
// 光标移动 / `\r` 覆盖 / 颜色 / 备用屏都在那里处理，这里只管滚动与聚焦。
import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../platform/terminal.dart';
import '../theme.dart';
import 'ansi.dart';

/// 回放缓冲上限（字符）：超出后只保留最近的尾部。
///
/// 每次轮询都要整段重解析（`parseAnsi` 是纯函数），所以这个值不能太大 ——
/// 12 万字符在 120 ms 的节奏下大约占几毫秒。
const int _maxBufferChars = 120000;
const int _keepBufferChars = 90000;

class TerminalView extends StatefulWidget {
  const TerminalView({
    super.key,
    this.startTerminal,
    this.readTerminal,
    this.writeTerminal,
    this.stopTerminal,
    this.pollInterval = const Duration(milliseconds: 120),
  });

  /// 数据源（测试注入；默认走 rust 桥接）。
  final TerminalStart Function()? startTerminal;
  final TerminalChunk Function()? readTerminal;
  final bool Function(String data)? writeTerminal;
  final void Function()? stopTerminal;
  final Duration pollInterval;

  @override
  State<TerminalView> createState() => _TerminalViewState();
}

class _TerminalViewState extends State<TerminalView> {
  final ScrollController _scroll = ScrollController();

  /// ConPTY 档的按键焦点（整块画面就是输入区）。
  final FocusNode _screenFocus = FocusNode();

  Timer? _poll;

  /// 屏幕缓冲（原始 ANSI 文本，含 `\r`；渲染时交给 `parseAnsi` 回放）。
  String _raw = '';

  /// `conpty` / `pipes`（空 = 还没起来）。
  String _mode = '';
  bool _alive = false;
  bool _focused = false;
  String _warning = '';
  String? _error;

  /// 管道档的行编辑缓冲（ConPTY 档不用它，shell 自己回显）。
  final TextEditingController _line = TextEditingController();

  bool get _pipe => _mode == 'pipes';

  @override
  void initState() {
    super.initState();
    _start();
  }

  @override
  void dispose() {
    _poll?.cancel();
    _line.dispose();
    _scroll.dispose();
    _screenFocus.dispose();
    // **不停 shell**：预览窗里切标签会卸载本组件，但终端是进程级单例 ——
    // 让它继续跑、继续缓冲输出，切回来时 `terminalStart` 会复用同一个进程。
    // 真正收掉它的是预览窗自身（`SandboxPanelState.dispose`）。
    super.dispose();
  }

  void _start() {
    final result = (widget.startTerminal ?? terminalStart)();
    setState(() {
      _error = result.ok ? null : result.error;
      _mode = result.mode;
      _alive = result.ok;
      _warning = result.ok ? result.warning : '';
      _raw = '';
      _line.clear();
    });
    _poll?.cancel();
    if (!result.ok) {
      return;
    }
    _poll = Timer.periodic(widget.pollInterval, (_) => _read());
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && !_pipe) {
        _screenFocus.requestFocus();
      }
    });
  }

  void _read() {
    final chunk = (widget.readTerminal ?? terminalRead)();
    if (!mounted) {
      return;
    }
    if (chunk.data.isEmpty && chunk.alive == _alive) {
      return;
    }
    setState(() {
      _alive = chunk.alive;
      if (chunk.mode.isNotEmpty) {
        _mode = chunk.mode;
      }
      if (chunk.data.isNotEmpty) {
        final next = _raw + chunk.data;
        _raw = next.length <= _maxBufferChars
            ? next
            : next.substring(next.length - _keepBufferChars);
      }
    });
    _scrollToBottom();
  }

  void _scrollToBottom() {
    if (!_scroll.hasClients) {
      return;
    }
    final position = _scroll.position;
    // 只在本来就贴着底部时跟着滚，避免用户向上翻看时被拽回。
    if (position.maxScrollExtent - position.pixels < 48) {
      position.jumpTo(position.maxScrollExtent);
    }
  }

  bool _write(String data) => (widget.writeTerminal ?? terminalWrite)(data);

  /// 管道档：Enter 才发整行（`cmd /K` 逐行执行）。
  void _submitLine() {
    if (!_alive) {
      return;
    }
    // Rust 侧会把这一行回显进缓冲（`cmd /Q` 自己不回显），界面上看得到自己敲的命令。
    _write('${_line.text}\r\n');
    _line.clear();
    _scrollToBottom();
  }

  /// ConPTY 档的逐键写回（方向键 / Ctrl+C / 退格都按终端语义发）。
  KeyEventResult _onKey(FocusNode node, KeyEvent event) {
    if (event is! KeyDownEvent || !_alive) {
      return KeyEventResult.ignored;
    }
    final key = event.logicalKey;
    if (HardwareKeyboard.instance.isControlPressed) {
      if (key == LogicalKeyboardKey.keyC) {
        _write('\x03');
        return KeyEventResult.handled;
      }
      return KeyEventResult.ignored;
    }
    if (key == LogicalKeyboardKey.enter ||
        key == LogicalKeyboardKey.numpadEnter) {
      _write('\r');
      return KeyEventResult.handled;
    }
    if (key == LogicalKeyboardKey.backspace) {
      _write('\x7f');
      return KeyEventResult.handled;
    }
    if (key == LogicalKeyboardKey.tab) {
      _write('\t');
      return KeyEventResult.handled;
    }
    final escapes = <LogicalKeyboardKey, String>{
      LogicalKeyboardKey.arrowUp: '\x1b[A',
      LogicalKeyboardKey.arrowDown: '\x1b[B',
      LogicalKeyboardKey.arrowRight: '\x1b[C',
      LogicalKeyboardKey.arrowLeft: '\x1b[D',
      LogicalKeyboardKey.home: '\x1b[H',
      LogicalKeyboardKey.end: '\x1b[F',
    };
    final escape = escapes[key];
    if (escape != null) {
      _write(escape);
      return KeyEventResult.handled;
    }
    final character = event.character;
    if (character != null && character.isNotEmpty) {
      _write(character);
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _buildToolbar(),
        Divider(height: 1, color: LunacColors.border),
        if (_warning.isNotEmpty) _buildNotice(),
        Expanded(child: _buildScreen()),
        if (_pipe && _error == null) _buildInputLine(),
      ],
    );
  }

  Widget _buildToolbar() {
    final label = switch (_mode) {
      'conpty' => 'ConPTY',
      'pipes' => '管道模式',
      _ => '未启动',
    };
    return Padding(
      padding: const EdgeInsets.fromLTRB(12, 4, 6, 4),
      child: Row(
        children: [
          Icon(Icons.terminal, size: 13, color: LunacColors.textMuted),
          const SizedBox(width: 6),
          Text('终端', style: TextStyle(color: LunacColors.text, fontSize: 12)),
          const SizedBox(width: 8),
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 1),
            decoration: BoxDecoration(
              color: LunacColors.surfaceHover,
              borderRadius: BorderRadius.circular(4),
            ),
            child: Text(
              _alive ? label : '已结束',
              style: TextStyle(
                color: _alive ? LunacColors.textDim : LunacColors.danger,
                fontSize: 9.5,
              ),
            ),
          ),
          const Spacer(),
          TextButton(
            onPressed: () => setState(_clearScreen),
            style: TextButton.styleFrom(
              foregroundColor: LunacColors.textDim,
              visualDensity: VisualDensity.compact,
            ),
            child: const Text('清屏', style: TextStyle(fontSize: 11)),
          ),
          TextButton(
            onPressed: _restart,
            style: TextButton.styleFrom(
              foregroundColor: LunacColors.accent,
              visualDensity: VisualDensity.compact,
            ),
            child: Text(
              _alive ? '重启' : '重新开始',
              style: const TextStyle(fontSize: 11),
            ),
          ),
        ],
      ),
    );
  }

  void _clearScreen() {
    _raw = '';
    _line.clear();
    if (_pipe && _alive) {
      // 管道档让 cmd 自己也清一次，免得下一次输出还带着旧提示符。
      _write('cls\r\n');
    }
  }

  void _restart() {
    (widget.stopTerminal ?? terminalStop)();
    _start();
  }

  Widget _buildNotice() {
    return Padding(
      padding: const EdgeInsets.fromLTRB(12, 6, 12, 0),
      child: Text(
        '$_warning\n管道模式仍可执行命令（cd / dir / 脚本都会保留状态），'
        '但没有全屏交互程序与颜色。',
        style: TextStyle(
          color: LunacColors.textMuted,
          fontSize: 10,
          height: 1.45,
        ),
      ),
    );
  }

  Widget _buildScreen() {
    if (_error != null) {
      return Center(
        child: Padding(
          padding: const EdgeInsets.all(16),
          child: Text(
            '终端启动失败：${_error!}',
            textAlign: TextAlign.center,
            style: TextStyle(color: LunacColors.danger, fontSize: 12),
          ),
        ),
      );
    }
    final base = TextStyle(
      color: ansiDefaultForeground,
      fontSize: 11.5,
      height: 1.35,
      fontFamily: 'Consolas',
      fontFamilyFallback: const ['Courier New', 'monospace'],
    );
    final spans = ansiSpansToTextSpan(parseAnsi(_raw), base: base);
    final Widget body = _pipe
        ? SelectableText.rich(spans)
        : Text.rich(
            TextSpan(
              children: [
                spans,
                TextSpan(
                  text: '▍',
                  style: TextStyle(
                    color: _focused ? LunacColors.accent : LunacColors.textMuted,
                  ),
                ),
              ],
            ),
          );
    final content = SingleChildScrollView(
      controller: _scroll,
      padding: const EdgeInsets.all(8),
      child: Align(alignment: Alignment.topLeft, child: body),
    );
    return Container(
      color: LunacColors.terminal,
      child: _pipe
          ? content
          : Focus(
              focusNode: _screenFocus,
              onKeyEvent: _onKey,
              onFocusChange: (focused) => setState(() => _focused = focused),
              child: GestureDetector(
                behavior: HitTestBehavior.opaque,
                onTap: () => _screenFocus.requestFocus(),
                child: content,
              ),
            ),
    );
  }

  Widget _buildInputLine() {
    return Container(
      decoration: BoxDecoration(
        color: LunacColors.terminal,
        border: Border(top: BorderSide(color: LunacColors.border)),
      ),
      padding: const EdgeInsets.fromLTRB(10, 2, 10, 4),
      child: Row(
        children: [
          Text(
            _alive ? '>' : '—',
            style: TextStyle(
              color: LunacColors.accent,
              fontSize: 12,
              fontFamily: 'Consolas',
            ),
          ),
          const SizedBox(width: 6),
          Expanded(
            child: TextField(
              controller: _line,
              enabled: _alive,
              style: TextStyle(
                color: LunacColors.text,
                fontSize: 12,
                fontFamily: 'Consolas',
                fontFamilyFallback: const ['Courier New', 'monospace'],
              ),
              cursorColor: LunacColors.accent,
              decoration: InputDecoration(
                isDense: true,
                border: InputBorder.none,
                hintText: _alive ? '输入命令，Enter 执行' : '会话已结束',
                hintStyle: TextStyle(
                  color: LunacColors.textMuted,
                  fontSize: 11,
                ),
              ),
              onSubmitted: (_) => _submitLine(),
            ),
          ),
          IconButton(
            tooltip: '执行',
            icon: const Icon(Icons.keyboard_return, size: 14),
            color: LunacColors.textDim,
            visualDensity: VisualDensity.compact,
            onPressed: _alive ? _submitLine : null,
          ),
        ],
      ),
    );
  }
}
