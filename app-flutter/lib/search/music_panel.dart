// 音乐面板（v1 `music` 插件「本地音乐」那一页的原生等价物）。
//
// 信息架构对齐 v1 当前最终效果（`app/src/plugins/builtin/music.ts` 的本地页）：
//   · 左栏（`renderLocalSide`）：本地音乐标题 + 「全部曲目」+ 每个曲库目录（带曲目数），
//     点一行 = 把主区过滤到那个目录；
//   · 主区（`renderLocalMain`）：筛选框 + 扫描 + 曲目表（序号 / 封面 / 标题 / 歌手·专辑 / 时长）；
//   · 底部播放条（`#music-lnowbar`）：顶部 2px 进度 + 正在播放 + 模式 / 上一首 / 播放暂停 /
//     下一首 / 停止 + 时间 / 音量；
//   · 播放界面（`#music-v-player`）：点「正在播放」进入，封面 + 歌名 + 滚动歌词 + 进度 / 控制，
//     返回歌单退出（Esc 也先回歌单）。
//
// 与 v1 的差异（如实降级，不放假按钮）：
//   · **Spotify 在线库已接入**（`music_spotify.rs`）：标题栏那颗按钮打开指挥台弹窗，
//     含 OAuth PKCE 授权 / 断开 / 正在播放 / 播放控制 / 设备接管 / 歌单 / 搜索；
//     未连接时可展开「使用自己的 Client ID」填 Client ID + 回调端口（与 v1 同一条路）；
//     **本机播放（librespot）按用户 2026-11-19 的裁决移除**，不启动子进程、不接本机 PCM；
//   · **歌词已接在线**（LRCLIB → LRCLIB 搜索 → 网易云兜底，与 v1 三级口径一致）；
//   · v1 左栏的「自定义队列」与 `.m3u/.m3u8/.pls` 导入：v2 落
//     `config/music-queues.json`（多条歌单），左栏在曲库目录之后列出，点一行只看该队列；
//   · v1 本地曲目没有歌词（`music.ts:1778-1780`），v2 按本次需求补上同名 `.lrc`；
//   · v1 关面板会停掉本地播放（`stopMusicPolling` 里的 `player_stop`），本面板
//     `dispose` 同样停止播放。
import 'dart:async';
import 'dart:io';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart' show ScrollDirection;
import 'package:flutter/services.dart';

import '../platform/music.dart';
import '../theme.dart';
import 'folder_picker.dart';
import 'tuning_page.dart';

/// 本地播放模式（v1 `localMode`；循环顺序 off → shuffle → repeat_all → repeat_one）。
enum _PlayMode { off, shuffle, repeatAll, repeatOne }

const double _lyricLineExtent = 32;

class MusicPanel extends StatefulWidget {
  const MusicPanel({
    super.key,
    required this.onClose,
    this.createEngine,
    this.loadLibrary,
    this.loadQueues,
    this.saveQueues,
    this.windowControls,
  });

  /// 关闭面板（Esc / 返回按钮）。
  final VoidCallback onClose;

  /// 独立窗的「置顶 / 最小化 / 关闭」（由 `plugin_window_main` 注入）。
  /// 非 null ⇒ 替换标题栏里的「返回」；null ⇒ 内嵌模式，保留返回。
  final Widget? windowControls;

  /// 测试注入用；null = 真实的 Rust 播放引擎（`rust/src/player.rs`）。
  final MusicAudioEngine Function()? createEngine;

  /// 测试注入用；null = 真实的 Rust 曲库读取。
  final Future<MusicLibrary?> Function()? loadLibrary;

  /// 测试注入用；null = 真实的 `config/music-queues.json` 读取。
  final List<MusicQueue>? Function()? loadQueues;

  /// 测试注入用；null = 真实的写盘（成功返回 null，失败返回一句人话）。
  final String? Function(List<MusicQueue> queues)? saveQueues;

  @override
  State<MusicPanel> createState() => _MusicPanelState();
}

class _MusicPanelState extends State<MusicPanel> {
  late final FocusNode _panelFocus = FocusNode(
    debugLabel: 'music-panel',
    onKeyEvent: _onKey,
  );

  final TextEditingController _filter = TextEditingController();
  final ScrollController _trackScroll = ScrollController();
  final ScrollController _lyricsScroll = ScrollController();

  MusicLibrary? _library;
  List<MusicTrack> _tracks = const [];

  /// 曲库目录（v1 形态：SQLite 的 `roots` 表，带曲目数 / 目录是否还在）。
  List<MediaRoot> _mediaRoots = const [];

  /// 命中总数（可能大于已加载的 `_tracks.length`；被截断时界面要如实说明）。
  int _totalTracks = 0;

  /// 扫描进度（后台线程 + 500ms 轮询）。
  MediaScanStatus _scan = MediaScanStatus.idle;
  Timer? _scanPoll;

  /// 当前正在浏览的播放列表（v1 `mediaList`：它**不是**一根目录，不进 `roots`）。
  String? _playlistName;
  String? _playlistPath;
  List<MusicTrack> _queue = const [];
  bool _loading = true;
  bool _unavailable = false;
  String? _loadError;

  MusicTrack? _currentTrack;
  String? _currentPath;

  /// 打开序号的令牌：快速连点两首时，后一次打开会作废前一次的回调。
  int _openSeq = 0;

  MusicAudioEngine? _engine;
  final List<StreamSubscription<dynamic>> _subscriptions = [];

  Duration _position = Duration.zero;
  Duration? _duration;
  double? _dragMs;
  bool _playing = false;
  double _volume = 0.8;
  _PlayMode _mode = _PlayMode.off;

  MusicLyrics? _lyrics;
  bool _lyricsLoading = false;
  int _activeLine = -1;
  bool _followSuspended = false;
  Timer? _followTimer;

  String? _notice;
  String? _playError;
  final List<int> _shuffleHistory = [];

  /// 曲库目录（v1 本地音乐「修改列表」的编辑态）与它的错误行。
  bool _managingRoots = false;
  String? _rootError;

  /// 左栏当前选中的目录（空串 = 「全部曲目」，与 v1 `mediaRootSel` 同口径）。
  String _selectedRoot = '';

  /// 自定义队列（v1 `localQueues` 的多条版）。null = 桥接不可用（左栏不画这一段）。
  List<MusicQueue>? _queues;

  /// 左栏当前选中的队列 id（null = 看媒体库；与目录选中互斥）。
  String? _activeQueueId;

  /// 「修改列表」编辑态里已点过一次、等第二下确认的队列 id。
  String? _queueArmedId;
  Timer? _queueArmTimer;
  String? _queueError;

  /// 左栏队列段下方的成功提示（v1 `music.local_queue_created` 那句话）。
  String? _queueNotice;

  /// v1 `#music-queue-dlg`「添加队列」小窗（名字 + 添加文件 + 已选清单 + 确定/取消）。
  bool _queueDialogOpen = false;
  final TextEditingController _queueName = TextEditingController();
  final List<String> _queueDraftFiles = [];
  String? _queueDialogError;

  /// v1 `queueEditOn`：自定义队列的「勾选歌曲 → 添加到队列」编辑态。
  bool _queuePickMode = false;

  /// 编辑态里勾中的曲目路径（小写，Windows 路径大小写不敏感）。
  final Set<String> _pickedPaths = {};

  /// 播放界面（v1 `#music-v-player`）：点播放条左侧「正在播放」进入，返回歌单退出。
  bool _playerView = false;

  /// 调音页（v1 `#music-v-tuning`）：标题栏那颗「调音」进入，Esc / ← 回本地音乐。
  bool _tuningOpen = false;

  /// 播放队列浮层（v1 `#music-lq-dd`）的开关。
  final MenuController _playQueueMenu = MenuController();

  List<String> get _roots => _library?.roots ?? const [];

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) {
        _panelFocus.requestFocus();
      }
    });
    unawaited(_loadLibrary());
    _loadQueues();
  }

  /// 读自定义队列（`config/music-queues.json`）；桥接缺失时左栏不画这一段。
  void _loadQueues() {
    final queues = (widget.loadQueues ?? readMusicQueues)();
    if (!mounted) {
      return;
    }
    setState(() {
      _queues = queues;
      if (_activeQueueId != null &&
          !(queues?.any((queue) => queue.id == _activeQueueId) ?? false)) {
        _activeQueueId = null;
      }
    });
  }

  /// 写自定义队列（测试可注入；成功返回 null）。
  String? _saveQueues(List<MusicQueue> queues) {
    final injected = widget.saveQueues;
    return injected != null ? injected(queues) : saveMusicQueues(queues);
  }

  @override
  void dispose() {
    _followTimer?.cancel();
    _queueArmTimer?.cancel();
    _scanPoll?.cancel();
    _queueName.dispose();
    for (final subscription in _subscriptions) {
      unawaited(subscription.cancel());
    }
    final engine = _engine;
    if (engine != null) {
      // v1 关面板 = 停掉本地播放（`stopMusicPolling` → `player_stop`）。
      // 停止 / 释放都吞掉异常：面板已经在拆，没有地方显示错误。
      unawaited(() async {
        try {
          await engine.stop();
        } catch (_) {
          // 没有会话 / 设备已释放都算正常。
        }
        try {
          await engine.dispose();
        } catch (_) {
          // 同上。
        }
      }());
    }
    _filter.dispose();
    _trackScroll.dispose();
    _lyricsScroll.dispose();
    _panelFocus.dispose();
    super.dispose();
  }

  KeyEventResult _onKey(FocusNode node, KeyEvent event) {
    if (event is KeyDownEvent &&
        event.logicalKey == LogicalKeyboardKey.escape) {
      // v1：小窗开着时 Esc 先关小窗，不退出插件（`music.ts:2189`）。
      if (_queueDialogOpen) {
        _closeQueueDialog();
        return KeyEventResult.handled;
      }
      // 播放界面里 Esc 先回歌单（v1 用标题栏的 ← 退出播放态）。
      if (_playerView) {
        setState(() => _playerView = false);
        return KeyEventResult.handled;
      }
      // 调音页同理：Esc 先回本地音乐（v1 调音页是插件窗里的一页）。
      if (_tuningOpen) {
        setState(() => _tuningOpen = false);
        return KeyEventResult.handled;
      }
      widget.onClose();
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  Future<void> _loadLibrary() async {
    setState(() {
      _loading = true;
      _loadError = null;
      // 重新读曲库 ⇒ 主区回到「曲库视图」（播放列表只是借主区显示一次）。
      _playlistName = null;
      _playlistPath = null;
    });
    // 测试注入（单测）优先：那条路不看宿主库。
    if (widget.loadLibrary != null) {
      final library = await widget.loadLibrary!();
      if (!mounted) {
        return;
      }
      setState(() {
        _loading = false;
        _unavailable = library == null;
        _library = library;
        _tracks = library?.tracks ?? const [];
        _totalTracks = _tracks.length;
        _mediaRoots = const [];
        _loadError = library?.error;
        _shuffleHistory.clear();
      });
      return;
    }
    // v1 形态：打开面板**只查库**（不重新解析几千个文件的标签）。
    final rootsReply = readMediaRoots();
    final page = readMediaTracks(limit: 2000);
    if (page.ok) {
      if (!mounted) {
        return;
      }
      final roots = rootsReply.roots ?? const <MediaRoot>[];
      setState(() {
        _loading = false;
        _unavailable = false;
        _mediaRoots = roots;
        _library = MusicLibrary(
          tracks: page.items ?? const [],
          roots: [for (final root in roots) root.path],
          error: null,
        );
        _tracks = page.items ?? const [];
        _totalTracks = page.total;
        _loadError = rootsReply.ok ? null : rootsReply.error;
        _shuffleHistory.clear();
      });
      return;
    }
    // 桥接不可用（或库打不开）⇒ 退回同步全量扫描，界面照旧能用。
    final library = await loadMusicLibrary();
    if (!mounted) {
      return;
    }
    setState(() {
      _loading = false;
      _unavailable = library == null;
      _library = library;
      _tracks = library?.tracks ?? const [];
      _totalTracks = _tracks.length;
      _mediaRoots = const [];
      _loadError = library?.error;
      _shuffleHistory.clear();
    });
  }

  /// v1「扫描」：起后台线程，界面 500ms 轮询进度。
  void _startScan() {
    final reply = startMediaScan(_selectedRoot);
    if (!reply.ok) {
      setState(() => _rootError = reply.error);
      return;
    }
    setState(() {
      _rootError = null;
      _notice = null;
      _scan = reply.status ?? MediaScanStatus.idle;
    });
    _scanPoll?.cancel();
    _scanPoll = Timer.periodic(
      const Duration(milliseconds: 500),
      (_) => _pollScan(),
    );
  }

  void _pollScan() {
    final status = mediaScanStatus();
    if (!mounted) {
      return;
    }
    setState(() => _scan = status);
    if (status.running) {
      return;
    }
    _scanPoll?.cancel();
    _scanPoll = null;
    if (status.error.isNotEmpty) {
      setState(() => _rootError = status.error);
    } else {
      setState(
        () => _notice =
            '扫描完成：新增 ${status.added} · 更新 ${status.updated} · '
            '移除 ${status.removed} · 未变 ${status.unchanged}',
      );
    }
    unawaited(_loadLibrary());
  }

  /// v1 `#music-local-add`：选一个音乐目录 → 写 `roots` 表。
  ///
  /// 2026-11-19：目录选择改成应用内 Flutter 目录浏览器（盘符 / 逐级 / 可手输 UNC），
  /// 添加成功后**自动扫描** —— 此前只写 roots，用户看到空列表找不到「扫描」入口
  /// （空状态那颗按钮还错接成了「重读库」），表现就是「本地音乐识别不出来」。
  Future<void> _addRoot() async {
    final dir = await pickFolderDialog(
      context,
      title: '选择音乐目录',
      initialPath: _roots.isNotEmpty ? _roots.last : '',
    );
    if (dir == null || dir.trim().isEmpty || !mounted) {
      return;
    }
    final reply = addMediaRoot(dir);
    if (!reply.ok) {
      setState(() => _rootError = reply.error);
      return;
    }
    setState(() {
      _rootError = null;
      _managingRoots = false;
      // 新目录 = 扫全库，别让左栏残留的目录选择把范围缩成一处。
      _selectedRoot = '';
      _activeQueueId = null;
      _notice = '目录已添加，正在扫描…';
    });
    await _loadLibrary();
    if (mounted) {
      _startScan();
    }
  }

  /// v1 编辑态的「移出列表」：删 `roots` 表里那一行（**连同它的曲目**）。
  Future<void> _removeRoot(String path) async {
    final reply = removeMediaRoot(path);
    if (!reply.ok) {
      setState(() => _rootError = reply.error);
      return;
    }
    setState(() => _rootError = null);
    await _loadLibrary();
  }

  /// v1 `openMediaPlaylist`：打开一份 `.m3u/.m3u8/.pls`，主区直接显示它的曲目。
  ///
  /// **它不动媒体库**：只读那个文件（可能指向没添加过的目录），不入库；
  /// 也不写进左栏 —— 播放列表不是一根目录，进了 `roots` 会把「扫描」的语义搅乱。
  Future<void> _openPlaylist() async {
    final path = pickMediaPlaylist();
    if (path.isEmpty) {
      return;
    }
    final reply = readMediaPlaylist(path);
    if (!mounted) {
      return;
    }
    if (!reply.ok) {
      setState(() {
        _rootError = reply.error == 'ERR_NOT_PLAYLIST'
            ? '这不是一份播放列表（只认 .m3u / .m3u8 / .pls）'
            : reply.error;
        _playlistName = null;
        _playlistPath = null;
      });
      return;
    }
    setState(() {
      _rootError = null;
      _playlistPath = path;
      _playlistName = reply.name;
      _tracks = reply.items ?? const [];
      _totalTracks = reply.total;
      // 左栏的选中态要退回去：主区现在显示的不是它了。
      _selectedRoot = '';
      _activeQueueId = null;
      _queuePickMode = false;
      _pickedPaths.clear();
    });
  }

  /// 从磁盘删除一首（v1 `media_delete_file`：文件 + 索引 + 封面缓存一起删）。
  Future<void> _deleteTrack(MusicTrack track) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('从磁盘删除', style: TextStyle(fontSize: 14)),
        content: Text(
          '会把文件删除，同时清掉曲库索引与封面缓存：\n${track.path}',
          style: const TextStyle(fontSize: 12),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('取消', style: TextStyle(fontSize: 12)),
          ),
          TextButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text('删除', style: TextStyle(fontSize: 12)),
          ),
        ],
      ),
    );
    if (ok != true) {
      return;
    }
    final reply = deleteMediaFile(track.path);
    if (!mounted) {
      return;
    }
    setState(() {
      _rootError = reply.ok ? null : reply.error;
      _notice = reply.ok ? '已删除「${track.title}」' : null;
    });
    if (reply.ok) {
      if (_currentPath == track.path) {
        unawaited(_stopPlayback());
        setState(() {
          _currentTrack = null;
          _currentPath = null;
        });
      }
      await _loadLibrary();
    }
  }

  // ── 自定义队列 ─────────────────────────────────────────────────

  /// 当前选中的队列（null = 看媒体库）。
  MusicQueue? get _activeQueue {
    final queues = _queues;
    final id = _activeQueueId;
    if (queues == null || id == null) {
      return null;
    }
    for (final queue in queues) {
      if (queue.id == id) {
        return queue;
      }
    }
    return null;
  }

  /// 左栏点队列：与点目录互斥（v1 `activeQueueId` / `mediaRootSel` 两套状态互斥）。
  void _selectQueue(MusicQueue queue) {
    setState(() {
      _activeQueueId = queue.id;
      _selectedRoot = '';
      _playlistName = null;
      _playlistPath = null;
      _queueArmedId = null;
      _queuePickMode = false;
      _pickedPaths.clear();
    });
  }

  /// v1 `#music-queue-dlg`：居中「添加队列」小窗（名字 + 添加文件 + 已选清单 + 确定/取消）。
  ///
  /// [seed] = 编辑态里勾中的曲目（v1 `openQueueDialog(root, seedFiles)` 的同一条路）。
  void _openQueueDialog({List<String> seed = const []}) {
    final seen = <String>{};
    final draft = <String>[];
    for (final path in seed) {
      if (path.isEmpty || !seen.add(path.toLowerCase())) {
        continue;
      }
      draft.add(path);
    }
    setState(() {
      _queueDialogOpen = true;
      _queueDialogError = null;
      _queueDraftFiles
        ..clear()
        ..addAll(draft);
      _queueName.clear();
      _queueNotice = null;
    });
  }

  /// 勾选 / 取消勾选一首（按路径小写比较；存的是原始路径，落盘不走样）。
  void _togglePick(MusicTrack track) {
    final key = track.path.toLowerCase();
    setState(() {
      if (_pickedPaths.any((path) => path.toLowerCase() == key)) {
        _pickedPaths.removeWhere((path) => path.toLowerCase() == key);
      } else {
        _pickedPaths.add(track.path);
      }
    });
  }

  bool _isPicked(String path) {
    final key = path.toLowerCase();
    return _pickedPaths.any((item) => item.toLowerCase() == key);
  }

  /// v1 本地页工具栏那枚列表按钮：进入 / 退出「自定义队列」勾选态。
  void _toggleQueuePickMode() {
    setState(() {
      _queuePickMode = !_queuePickMode;
      if (!_queuePickMode) {
        _pickedPaths.clear();
      }
    });
  }

  void _closeQueueDialog() {
    setState(() {
      _queueDialogOpen = false;
      _queueDialogError = null;
    });
  }

  /// 小窗里「添加文件…」：多选音频；已选过的按路径小写去重（v1 同口径）。
  void _pickQueueFiles() {
    final picked = pickMusicFiles();
    if (!mounted || picked.isEmpty) {
      return;
    }
    final seen = {for (final path in _queueDraftFiles) path.toLowerCase()};
    setState(() {
      for (final path in picked) {
        if (seen.add(path.toLowerCase())) {
          _queueDraftFiles.add(path);
        }
      }
      _queueDialogError = null;
    });
  }

  /// 小窗「确定」：至少一个文件才放行（v1 `music.local_queue_need_files`）。
  Future<void> _submitQueueDialog() async {
    if (_queueDraftFiles.isEmpty) {
      setState(() => _queueDialogError = '请先添加至少一个文件');
      return;
    }
    final name = _queueName.text.trim();
    final label = name.isEmpty ? '未命名队列' : name;
    final ok = await _appendQueue(name: label, paths: [..._queueDraftFiles]);
    if (!mounted) {
      return;
    }
    if (!ok) {
      setState(() => _queueDialogError = '保存队列失败');
      return;
    }
    setState(() {
      _queueDialogOpen = false;
      _queueDialogError = null;
      _queueNotice = '已创建队列「$label」';
      // v1：创建完就退出勾选态。
      _queuePickMode = false;
      _pickedPaths.clear();
    });
  }

  /// 导入 `.m3u/.m3u8/.pls` 为一条新队列（v1 左栏的播放列表导入）。
  Future<void> _importQueuePlaylist() async {
    final imported = importMusicPlaylist();
    if (!mounted) {
      return;
    }
    if (imported == null) {
      setState(() => _queueError = '不可用（rust 桥接未加载）');
      return;
    }
    if (imported.canceled) {
      return;
    }
    if (!imported.ok) {
      setState(() {
        _queueError = imported.error.isEmpty ? '导入播放列表失败' : imported.error;
      });
      return;
    }
    await _appendQueue(name: imported.name, paths: imported.paths);
  }

  /// 落盘一条新队列；返回是否成功（失败时 `_queueError` 已写好人话）。
  Future<bool> _appendQueue({
    required String name,
    required List<String> paths,
  }) async {
    final queues = _queues;
    if (queues == null) {
      setState(() => _queueError = '不可用（rust 桥接未加载）');
      return false;
    }
    final id = 'queue-${DateTime.now().millisecondsSinceEpoch}';
    final label = name.trim().isEmpty ? '未命名队列' : name.trim();
    final error = _saveQueues([
      ...queues,
      MusicQueue(id: id, name: label, paths: paths),
    ]);
    if (!mounted) {
      return false;
    }
    if (error != null) {
      setState(() => _queueError = error);
      return false;
    }
    setState(() {
      _queueError = null;
      _activeQueueId = id;
      _selectedRoot = '';
    });
    _loadQueues();
    return true;
  }

  /// 删除一条队列：两段式确认（与 v1 的编辑态口径一致，不弹原生确认框）。
  void _removeQueue(MusicQueue queue) {
    final queues = _queues;
    if (queues == null) {
      return;
    }
    if (_queueArmedId != queue.id) {
      _queueArmTimer?.cancel();
      setState(() => _queueArmedId = queue.id);
      _queueArmTimer = Timer(const Duration(seconds: 3), () {
        if (mounted) {
          setState(() => _queueArmedId = null);
        }
      });
      return;
    }
    _queueArmTimer?.cancel();
    final error = _saveQueues([
      for (final item in queues)
        if (item.id != queue.id) item,
    ]);
    if (!mounted) {
      return;
    }
    if (error != null) {
      setState(() {
        _queueError = error;
        _queueArmedId = null;
      });
      return;
    }
    setState(() {
      _queueError = null;
      _queueArmedId = null;
      if (_activeQueueId == queue.id) {
        _activeQueueId = null;
      }
    });
    _loadQueues();
  }

  MusicAudioEngine _ensureEngine() {
    final existing = _engine;
    if (existing != null) {
      return existing;
    }
    final engine = (widget.createEngine ?? () => RustMusicEngine())();
    _engine = engine;
    _subscriptions.add(
      engine.positionStream.listen((position) => _onPosition(position)),
    );
    _subscriptions.add(
      engine.playingStream.listen((playing) {
        if (mounted) {
          setState(() => _playing = playing);
        }
      }),
    );
    _subscriptions.add(
      engine.durationStream.listen((duration) {
        if (mounted && duration != null && duration > Duration.zero) {
          setState(() => _duration = duration);
        }
      }),
    );
    _subscriptions.add(engine.completedStream.listen((_) => _onCompleted()));
    return engine;
  }

  void _onPosition(Duration position) {
    if (!mounted) {
      return;
    }
    final index = _lyricIndexAt(position.inMilliseconds);
    final changed = index != _activeLine;
    setState(() {
      _position = position;
      if (changed) {
        _activeLine = index;
      }
    });
    if (changed) {
      _scrollToActiveLine();
    }
  }

  // ── 播放 ───────────────────────────────────────────────────────

  void _playFromList(MusicTrack track) {
    // 队列 = 点播那一刻屏幕上的曲目（v1 `localQueue` 同口径）。
    _queue = List<MusicTrack>.of(_filteredTracks);
    _shuffleHistory.clear();
    unawaited(_playTrack(track));
  }

  Future<void> _playTrack(MusicTrack track) async {
    final engine = _ensureEngine();
    final seq = ++_openSeq;
    setState(() {
      _currentTrack = track;
      _currentPath = track.path;
      _position = Duration.zero;
      _dragMs = null;
      _duration = track.durationMs > 0
          ? Duration(milliseconds: track.durationMs)
          : null;
      _notice = null;
      _playError = null;
      _lyrics = null;
      _lyricsLoading = true;
      _activeLine = -1;
    });
    unawaited(_loadLyricsFor(track));
    try {
      final duration = await engine.open(track.path);
      if (!mounted || seq != _openSeq || _currentPath != track.path) {
        return;
      }
      if (duration != null && duration > Duration.zero) {
        setState(() => _duration = duration);
      }
      await engine.setVolume(_volume);
      await engine.play();
    } catch (e) {
      if (!mounted) {
        return;
      }
      setState(() {
        _lyricsLoading = false;
        _playError = _playErrorText(e);
      });
    }
  }

  Future<void> _togglePlay() async {
    final track = _currentTrack;
    if (track == null) {
      return;
    }
    final engine = _ensureEngine();
    try {
      if (_playing) {
        await engine.pause();
        return;
      }
      // 放完了 / 停在末尾 ⇒ 从头重放这一首（v1 `toggleLocalPause` 同语义）。
      final duration = _duration;
      if (duration != null &&
          duration > Duration.zero &&
          _position >= duration) {
        await engine.seek(Duration.zero);
      }
      await engine.play();
    } catch (e) {
      if (mounted) {
        setState(() => _playError = _playErrorText(e));
      }
    }
  }

  Future<void> _stopPlayback() async {
    final engine = _engine;
    if (engine == null) {
      return;
    }
    try {
      await engine.pause();
      await engine.seek(Duration.zero);
    } catch (e) {
      if (mounted) {
        setState(() => _playError = _playErrorText(e));
      }
    }
    if (!mounted) {
      return;
    }
    setState(() {
      _position = Duration.zero;
      _dragMs = null;
      _playing = false;
    });
  }

  void _onCompleted() {
    unawaited(_advance(step: 1, auto: true));
  }

  /// 上一首 / 下一首 / 自动续播（v1 `advanceLocal` 的规则逐条照搬）。
  Future<void> _advance({required int step, required bool auto}) async {
    final queue = _queue.isNotEmpty ? _queue : _tracks;
    if (queue.isEmpty) {
      return;
    }
    var index = _currentPath == null
        ? -1
        : queue.indexWhere((track) => track.path == _currentPath);
    if (index < 0) {
      if (!auto) {
        await _playTrack(queue.first);
      }
      return;
    }
    final n = queue.length;
    int next;
    if (auto && _mode == _PlayMode.repeatOne) {
      next = index;
    } else if (_mode == _PlayMode.shuffle && n > 1) {
      if (step < 0 && _shuffleHistory.isNotEmpty) {
        next = _shuffleHistory.removeLast();
      } else {
        final random = math.Random();
        do {
          next = random.nextInt(n);
        } while (next == index);
        _shuffleHistory.add(index);
      }
    } else {
      next = index + step;
    }
    if (_mode == _PlayMode.repeatAll || _mode == _PlayMode.shuffle) {
      next = ((next % n) + n) % n;
    } else if (next < 0 || next >= n) {
      if (!auto) {
        setState(() => _notice = '已经是这一批的最后一首了');
      } else {
        await _engine?.pause();
      }
      return;
    }
    await _playTrack(queue[next]);
  }

  Future<void> _seekTo(int milliseconds) async {
    final engine = _engine;
    if (engine == null || _currentTrack == null) {
      return;
    }
    final position = Duration(milliseconds: math.max(0, milliseconds));
    try {
      await engine.seek(position);
    } catch (e) {
      if (mounted) {
        setState(() => _playError = _playErrorText(e));
      }
      return;
    }
    if (!mounted) {
      return;
    }
    _onPosition(position);
  }

  Future<void> _loadLyricsFor(MusicTrack track) async {
    final local = await loadMusicLyrics(track.path);
    if (!mounted || _currentPath != track.path) {
      return;
    }
    // 本地没有同名 .lrc（且不是读盘失败）⇒ 走在线三级取词（LRCLIB → 搜索 → 网易云）。
    if (local.isEmpty && local.error == null && track.title.trim().isNotEmpty) {
      setState(() {
        _lyrics = null;
        _lyricsLoading = true;
      });
      final online = await loadOnlineMusicLyrics(
        title: track.title,
        artist: track.artist,
        album: track.album,
        duration: track.durationMs / 1000.0,
      );
      if (!mounted || _currentPath != track.path) {
        return;
      }
      setState(() {
        _lyricsLoading = false;
        _lyrics = online;
        _activeLine = _lyricIndexAt(_position.inMilliseconds);
      });
      return;
    }
    setState(() {
      _lyricsLoading = false;
      _lyrics = local;
      _activeLine = _lyricIndexAt(_position.inMilliseconds);
    });
  }

  /// 手动重试在线取词（本地没有 .lrc 时的兜底入口）。
  Future<void> _retryOnlineLyrics() async {
    final track = _currentTrack;
    if (track == null || _lyricsLoading) {
      return;
    }
    setState(() {
      _lyrics = null;
      _lyricsLoading = true;
    });
    final online = await loadOnlineMusicLyrics(
      title: track.title,
      artist: track.artist,
      album: track.album,
      duration: track.durationMs / 1000.0,
    );
    if (!mounted || _currentPath != track.path) {
      return;
    }
    setState(() {
      _lyricsLoading = false;
      _lyrics = online;
      _activeLine = _lyricIndexAt(_position.inMilliseconds);
    });
  }

  /// 打开 Spotify 在线库弹层（连接 / 播放控制 / 歌单 / 搜索 / 设备）。
  Future<void> _openSpotifySheet() async {
    await showDialog<void>(
      context: context,
      barrierColor: Colors.black54,
      builder: (context) => const _SpotifySheet(),
    );
  }

  int _lyricIndexAt(int milliseconds) {
    final lines = _lyrics?.lines ?? const <MusicLyricLine>[];
    var index = -1;
    for (var i = 0; i < lines.length; i++) {
      if (lines[i].timeMs <= milliseconds) {
        index = i;
      } else {
        break;
      }
    }
    return index;
  }

  void _scrollToActiveLine() {
    if (_followSuspended || _activeLine < 0) {
      return;
    }
    if (!_lyricsScroll.hasClients) {
      return;
    }
    final target = (_activeLine * _lyricLineExtent - _lyricLineExtent * 2)
        .clamp(0.0, _lyricsScroll.position.maxScrollExtent);
    _lyricsScroll.animateTo(
      target,
      duration: const Duration(milliseconds: 320),
      curve: Curves.easeOutCubic,
    );
  }

  /// 用户手动翻歌词时暂停跟随，3 秒后回到当前行（v1 `suspendFollow`）。
  bool _onLyricsScroll(ScrollNotification notification) {
    if (notification is UserScrollNotification &&
        notification.direction != ScrollDirection.idle) {
      _followSuspended = true;
      _followTimer?.cancel();
      _followTimer = Timer(const Duration(seconds: 3), () {
        if (!mounted) {
          return;
        }
        _followSuspended = false;
        _activeLine = _lyricIndexAt(_position.inMilliseconds);
        _scrollToActiveLine();
      });
    }
    return false;
  }

  void _cycleMode() {
    const order = [
      _PlayMode.off,
      _PlayMode.shuffle,
      _PlayMode.repeatAll,
      _PlayMode.repeatOne,
    ];
    final next = order[(order.indexOf(_mode) + 1) % order.length];
    setState(() => _mode = next);
  }

  String get _modeTitle => switch (_mode) {
    _PlayMode.off => '随机播放：关',
    _PlayMode.shuffle => '随机播放：开',
    _PlayMode.repeatAll => '循环列表',
    _PlayMode.repeatOne => '单曲循环',
  };

  IconData get _modeIcon => switch (_mode) {
    _PlayMode.off => Icons.shuffle,
    _PlayMode.shuffle => Icons.shuffle,
    _PlayMode.repeatAll => Icons.repeat,
    _PlayMode.repeatOne => Icons.repeat_one,
  };

  // ── 界面 ───────────────────────────────────────────────────────

  List<MusicTrack> get _filteredTracks {
    final needle = _filter.text.trim().toLowerCase();
    final root = _selectedRoot.toLowerCase();
    final queuePaths = _activeQueue == null
        ? null
        : {for (final path in _activeQueue!.paths) path.toLowerCase()};
    return _tracks.where((track) {
      // 左栏选了某条自定义队列 ⇒ 只看它里面的曲目（v1 `activeQueueId`）。
      if (queuePaths != null) {
        if (!queuePaths.contains(track.path.toLowerCase())) {
          return false;
        }
      } else if (root.isNotEmpty &&
          !track.path.toLowerCase().startsWith(root)) {
        // 左栏选了某个目录 ⇒ 只看它下面的曲目（v1 `mediaRootSel`）。
        return false;
      }
      if (needle.isEmpty) {
        return true;
      }
      return track.title.toLowerCase().contains(needle) ||
          track.artist.toLowerCase().contains(needle) ||
          track.album.toLowerCase().contains(needle);
    }).toList();
  }

  /// 某个目录下的曲目数（左栏行尾的「N 首」）。
  ///
  /// 优先用库里的计数（`roots.tracks`）—— 面板只加载前 2000 首，
  /// 按已加载的曲目数前缀统计会在超大曲库上少报。
  int _rootCount(String root) {
    for (final entry in _mediaRoots) {
      if (entry.path == root) {
        return entry.tracks;
      }
    }
    final prefix = root.toLowerCase();
    return _tracks
        .where((track) => track.path.toLowerCase().startsWith(prefix))
        .length;
  }

  /// 目录名（v1 左栏显示 `r.name || r.path`；v2 只取最后一段）。
  String _rootName(String root) {
    final trimmed = root.replaceAll(RegExp(r'[\\/]+$'), '');
    final cut = trimmed.lastIndexOf(RegExp(r'[\\/]'));
    return cut >= 0 ? trimmed.substring(cut + 1) : trimmed;
  }

  /// 这个目录现在还在不在（库里标了 `missing`）。
  bool _rootMissing(String root) {
    for (final entry in _mediaRoots) {
      if (entry.path == root) {
        return entry.missing;
      }
    }
    return false;
  }

  @override
  Widget build(BuildContext context) {
    return Focus(
      focusNode: _panelFocus,
      autofocus: true,
      onKeyEvent: _onKey,
      child: Stack(
        fit: StackFit.expand,
        children: [
          Container(
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
            child: _tuningOpen
                ? TuningPage(
                    onClose: () => setState(() => _tuningOpen = false),
                  )
                : Column(
                    children: [
                      _buildHeader(),
                      Expanded(
                        child: Row(
                          crossAxisAlignment: CrossAxisAlignment.stretch,
                          children: [
                            SizedBox(width: 236, child: _buildSidebar()),
                            Container(width: 1, color: LunacColors.border),
                            // v1 本地页 = 左栏 + 曲目表 + 底部播放条；点播放条进入播放界面。
                            Expanded(
                              child: _playerView
                                  ? _buildPlayerView()
                                  : _buildListMain(),
                            ),
                          ],
                        ),
                      ),
                      if (!_playerView && _currentTrack != null) _buildNowBar(),
                    ],
                  ),
          ),
          if (_queueDialogOpen) _buildQueueDialog(),
        ],
      ),
    );
  }

  /// v1 `.music-dlg-mask` + `.music-dlg`：页面正中的「添加队列」小窗。
  Widget _buildQueueDialog() {
    return Positioned.fill(
      child: Container(
        color: Colors.black.withAlpha(120),
        alignment: Alignment.center,
        child: Container(
          width: 380,
          padding: const EdgeInsets.fromLTRB(16, 14, 16, 10),
          decoration: BoxDecoration(
            color: LunacColors.surface,
            border: Border.all(color: LunacColors.border),
            borderRadius: BorderRadius.circular(10),
          ),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Text(
                '添加队列',
                style: TextStyle(
                  color: LunacColors.text,
                  fontSize: 13.5,
                  fontWeight: FontWeight.w600,
                ),
              ),
              const SizedBox(height: 10),
              Text(
                '队列名称',
                style: TextStyle(color: LunacColors.textDim, fontSize: 11),
              ),
              const SizedBox(height: 4),
              SizedBox(
                height: 30,
                child: TextField(
                  controller: _queueName,
                  style: TextStyle(color: LunacColors.text, fontSize: 12),
                  cursorColor: LunacColors.accent,
                  decoration: InputDecoration(
                    isDense: true,
                    filled: true,
                    fillColor: LunacColors.background,
                    hintText: '给这个队列起个名字',
                    hintStyle: TextStyle(
                      color: LunacColors.textMuted,
                      fontSize: 11.5,
                    ),
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
              const SizedBox(height: 10),
              Row(
                children: [
                  TextButton.icon(
                    onPressed: _pickQueueFiles,
                    icon: const Icon(Icons.add, size: 14),
                    label: const Text(
                      '添加文件…',
                      style: TextStyle(fontSize: 11.5),
                    ),
                    style: TextButton.styleFrom(
                      foregroundColor: LunacColors.textDim,
                      visualDensity: VisualDensity.compact,
                      minimumSize: Size.zero,
                      padding: const EdgeInsets.symmetric(horizontal: 8),
                    ),
                  ),
                  const SizedBox(width: 6),
                  Expanded(
                    child: Text(
                      '可跨不同文件夹多选，重复的自动去重',
                      style: TextStyle(
                        color: LunacColors.textMuted,
                        fontSize: 10,
                      ),
                    ),
                  ),
                ],
              ),
              const SizedBox(height: 6),
              Container(
                height: 140,
                decoration: BoxDecoration(
                  color: LunacColors.background,
                  border: Border.all(color: LunacColors.border),
                  borderRadius: BorderRadius.circular(6),
                ),
                child: _buildQueueDraftList(),
              ),
              const SizedBox(height: 6),
              if (_queueDialogError != null)
                Align(
                  alignment: Alignment.centerRight,
                  child: Text(
                    _queueDialogError!,
                    style: TextStyle(color: LunacColors.danger, fontSize: 10.5),
                  ),
                ),
              const SizedBox(height: 4),
              Row(
                mainAxisAlignment: MainAxisAlignment.end,
                children: [
                  TextButton(
                    onPressed: _closeQueueDialog,
                    style: TextButton.styleFrom(
                      foregroundColor: LunacColors.textDim,
                      visualDensity: VisualDensity.compact,
                    ),
                    child: const Text('取消', style: TextStyle(fontSize: 12)),
                  ),
                  const SizedBox(width: 6),
                  TextButton(
                    onPressed: () => unawaited(_submitQueueDialog()),
                    style: TextButton.styleFrom(
                      foregroundColor: LunacColors.accent,
                      visualDensity: VisualDensity.compact,
                    ),
                    child: const Text('确定', style: TextStyle(fontSize: 12)),
                  ),
                ],
              ),
            ],
          ),
        ),
      ),
    );
  }

  /// 小窗里的「已选文件」清单（逐条可移除）。
  Widget _buildQueueDraftList() {
    if (_queueDraftFiles.isEmpty) {
      return Center(
        child: Text(
          '还没有选择文件',
          style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
        ),
      );
    }
    return ListView.builder(
      padding: const EdgeInsets.symmetric(vertical: 4),
      itemCount: _queueDraftFiles.length + 1,
      itemBuilder: (context, index) {
        if (index == 0) {
          // v1 `music.local_queue_files_n`：清单顶部那一行「已选 N 个文件」。
          return Padding(
            padding: const EdgeInsets.fromLTRB(10, 2, 8, 4),
            child: Text(
              '已选 ${_queueDraftFiles.length} 个文件',
              style: TextStyle(color: LunacColors.textMuted, fontSize: 10.5),
            ),
          );
        }
        final path = _queueDraftFiles[index - 1];
        return Padding(
          padding: const EdgeInsets.fromLTRB(10, 1, 2, 1),
          child: Row(
            children: [
              Icon(Icons.music_note, size: 13, color: LunacColors.textMuted),
              const SizedBox(width: 6),
              Expanded(
                child: Tooltip(
                  message: path,
                  child: Text(
                    _pathName(path),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: LunacColors.textDim,
                      fontSize: 11.5,
                    ),
                  ),
                ),
              ),
              IconButton(
                tooltip: '移除',
                icon: const Icon(Icons.close, size: 12),
                color: LunacColors.textMuted,
                visualDensity: VisualDensity.compact,
                constraints: const BoxConstraints.tightFor(
                  width: 22,
                  height: 22,
                ),
                padding: EdgeInsets.zero,
                onPressed: () =>
                    setState(() => _queueDraftFiles.removeAt(index)),
              ),
            ],
          ),
        );
      },
    );
  }

  /// 路径最后一段（小窗清单只显示文件名，完整路径放 tooltip）。
  String _pathName(String path) {
    final cut = path.lastIndexOf(RegExp(r'[\\/]'));
    return cut >= 0 ? path.substring(cut + 1) : path;
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
            '音乐',
            style: TextStyle(
              color: LunacColors.text,
              fontSize: 14,
              fontWeight: FontWeight.w600,
            ),
          ),
          const SizedBox(width: 8),
          Text(
            '本地媒体库 · 在线歌词（LRCLIB / 网易云）',
            style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
          ),
          const Spacer(),
          if (_tracks.isNotEmpty)
            Text(
              '${_tracks.length} 首',
              style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
            ),
          const SizedBox(width: 10),
          // v1 标题栏三段切换的第三段就是调音页（`#music-v-tuning`）。
          TextButton.icon(
            onPressed: () => setState(() => _tuningOpen = true),
            icon: const Icon(Icons.graphic_eq, size: 13),
            label: const Text('调音', style: TextStyle(fontSize: 10.5)),
            style: TextButton.styleFrom(
              foregroundColor: LunacColors.textDim,
              padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
              minimumSize: Size.zero,
              visualDensity: VisualDensity.compact,
            ),
          ),
          const SizedBox(width: 4),
          // v1 顶部是 Spotify 搜索框；v2 把在线库收进一个弹层（连接 / 歌单 / 搜索 / 设备）。
          TextButton.icon(
            onPressed: _openSpotifySheet,
            icon: const Icon(Icons.cloud_outlined, size: 13),
            label: const Text('Spotify', style: TextStyle(fontSize: 10.5)),
            style: TextButton.styleFrom(
              foregroundColor: LunacColors.textDim,
              padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
              minimumSize: Size.zero,
              visualDensity: VisualDensity.compact,
            ),
          ),
          // 独立窗：置顶 / 最小化 / 关闭 常驻右上角。
          if (widget.windowControls != null) ...[
            const SizedBox(width: 6),
            widget.windowControls!,
          ],
        ],
      ),
    );
  }

  /// 左栏（v1 `renderLocalSide`）：标题 + 「全部曲目」+ 每个曲库目录。
  Widget _buildSidebar() {
    final roots = _roots;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // v1 `.music-local-head`：标题 + 「+ 添加音乐目录」+ 「修改列表」。
        Padding(
          padding: const EdgeInsets.fromLTRB(12, 2, 6, 2),
          child: Row(
            children: [
              Text(
                '本地音乐',
                style: TextStyle(
                  color: LunacColors.text,
                  fontSize: 12,
                  fontWeight: FontWeight.w600,
                ),
              ),
              const Spacer(),
              IconButton(
                tooltip: '添加音乐目录',
                icon: const Icon(Icons.add, size: 15),
                color: LunacColors.textDim,
                visualDensity: VisualDensity.compact,
                constraints: const BoxConstraints.tightFor(
                  width: 28,
                  height: 26,
                ),
                padding: EdgeInsets.zero,
                onPressed: _addRoot,
              ),
              IconButton(
                tooltip: _managingRoots ? '退出并保存' : '修改列表',
                icon: Icon(
                  _managingRoots ? Icons.check : Icons.edit_outlined,
                  size: 14,
                ),
                color: _managingRoots
                    ? LunacColors.accent
                    : LunacColors.textDim,
                visualDensity: VisualDensity.compact,
                constraints: const BoxConstraints.tightFor(
                  width: 28,
                  height: 26,
                ),
                padding: EdgeInsets.zero,
                onPressed: () =>
                    setState(() => _managingRoots = !_managingRoots),
              ),
            ],
          ),
        ),
        if (_managingRoots) _buildRootEditor(),
        if (_scan.running)
          Padding(
            padding: const EdgeInsets.fromLTRB(12, 0, 12, 4),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                LinearProgressIndicator(
                  value: _scan.total > 0 ? _scan.progress : null,
                  minHeight: 3,
                ),
                const SizedBox(height: 2),
                Text(
                  _scan.total > 0
                      ? '扫描中 ${_scan.scanned}/${_scan.total}'
                      : '正在统计文件…',
                  style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
                ),
              ],
            ),
          ),
        if (_rootError != null)
          Padding(
            padding: const EdgeInsets.fromLTRB(12, 0, 12, 4),
            child: Text(
              _rootError!,
              style: TextStyle(color: LunacColors.danger, fontSize: 10),
            ),
          ),
        Expanded(
          child: ListView(
            padding: const EdgeInsets.fromLTRB(6, 2, 6, 8),
            children: [
              _buildSideRow(
                selected: _selectedRoot.isEmpty && _activeQueueId == null,
                icon: Icons.library_music_outlined,
                name: '全部曲目',
                count: _totalTracks > 0 ? _totalTracks : _tracks.length,
                onTap: () => setState(() {
                  _selectedRoot = '';
                  _activeQueueId = null;
                  _playlistName = null;
                  _playlistPath = null;
                  _queuePickMode = false;
                  _pickedPaths.clear();
                }),
              ),
              if (!_loading && !_unavailable && roots.isEmpty)
                Padding(
                  padding: EdgeInsets.fromLTRB(10, 8, 10, 0),
                  child: Text(
                    '还没有添加目录',
                    style: TextStyle(
                      color: LunacColors.textMuted,
                      fontSize: 10.5,
                    ),
                  ),
                ),
              for (final root in roots)
                _buildSideRow(
                  selected: _activeQueueId == null && _selectedRoot == root,
                  icon: Icons.folder_outlined,
                  // 被删 / 拔盘的根要能看出来（v1 `MediaRootDto.missing`）。
                  name:
                      '${_rootName(root)}${_rootMissing(root) ? '（目录不存在）' : ''}',
                  detail: root,
                  count: _rootCount(root),
                  onTap: () => setState(() {
                    _selectedRoot = root;
                    _activeQueueId = null;
                    _playlistName = null;
                    _playlistPath = null;
                    _queuePickMode = false;
                    _pickedPaths.clear();
                  }),
                ),
              if (_queues != null) _buildQueueSection(),
            ],
          ),
        ),
      ],
    );
  }

  /// 左栏「自定义队列」段（v1 `renderLocalQueues`：标题 + 若干行，空表不画提示）。
  Widget _buildQueueSection() {
    final queues = _queues ?? const <MusicQueue>[];
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(10, 10, 4, 2),
          child: Row(
            children: [
              Text(
                '自定义队列',
                style: TextStyle(
                  color: LunacColors.textDim,
                  fontSize: 11,
                  fontWeight: FontWeight.w600,
                ),
              ),
              const Spacer(),
              IconButton(
                tooltip: '导入播放列表（.m3u / .m3u8 / .pls）',
                icon: const Icon(Icons.playlist_add, size: 15),
                color: LunacColors.textDim,
                visualDensity: VisualDensity.compact,
                constraints: const BoxConstraints.tightFor(
                  width: 26,
                  height: 24,
                ),
                padding: EdgeInsets.zero,
                onPressed: _importQueuePlaylist,
              ),
              IconButton(
                tooltip: '添加队列（名字 + 选文件）',
                icon: const Icon(Icons.add, size: 15),
                color: LunacColors.textDim,
                visualDensity: VisualDensity.compact,
                constraints: const BoxConstraints.tightFor(
                  width: 26,
                  height: 24,
                ),
                padding: EdgeInsets.zero,
                onPressed: _openQueueDialog,
              ),
            ],
          ),
        ),
        if (_queueError != null)
          Padding(
            padding: const EdgeInsets.fromLTRB(10, 0, 8, 4),
            child: Text(
              _queueError!,
              style: TextStyle(color: LunacColors.danger, fontSize: 10),
            ),
          ),
        if (_queueNotice != null)
          Padding(
            padding: const EdgeInsets.fromLTRB(10, 0, 8, 4),
            child: Text(
              _queueNotice!,
              style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
            ),
          ),
        for (final queue in queues)
          _buildSideRow(
            selected: _activeQueueId == queue.id,
            icon: Icons.queue_music,
            name: queue.name.isEmpty ? '未命名队列' : queue.name,
            count: queue.paths.length,
            onTap: () => _selectQueue(queue),
            trailing: _managingRoots
                ? IconButton(
                    tooltip: _queueArmedId == queue.id ? '再点一次删除' : '删除队列',
                    icon: Icon(
                      _queueArmedId == queue.id ? Icons.delete : Icons.close,
                      size: 13,
                    ),
                    color: _queueArmedId == queue.id
                        ? LunacColors.danger
                        : LunacColors.textMuted,
                    visualDensity: VisualDensity.compact,
                    constraints: const BoxConstraints.tightFor(
                      width: 22,
                      height: 22,
                    ),
                    padding: EdgeInsets.zero,
                    onPressed: () => _removeQueue(queue),
                  )
                : null,
          ),
      ],
    );
  }

  /// 左栏一行（v1 `.music-pl`：封面位 + 名称 / 说明 + 曲目数）。
  Widget _buildSideRow({
    required bool selected,
    required IconData icon,
    required String name,
    String? detail,
    required int count,
    required VoidCallback onTap,
    Widget? trailing,
  }) {
    return InkWell(
      borderRadius: BorderRadius.circular(6),
      onTap: onTap,
      child: Container(
        margin: const EdgeInsets.only(bottom: 2),
        padding: const EdgeInsets.fromLTRB(8, 5, 8, 5),
        decoration: BoxDecoration(
          color: selected ? LunacColors.surfaceHover : null,
          borderRadius: BorderRadius.circular(6),
        ),
        child: Row(
          children: [
            Container(
              width: 24,
              height: 24,
              decoration: BoxDecoration(
                color: LunacColors.background,
                borderRadius: BorderRadius.circular(5),
              ),
              child: Icon(
                icon,
                size: 13,
                color: selected ? LunacColors.accent : LunacColors.textMuted,
              ),
            ),
            const SizedBox(width: 8),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    name,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: selected ? LunacColors.text : LunacColors.textDim,
                      fontSize: 12,
                    ),
                  ),
                  if (detail != null)
                    Text(
                      detail,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        color: LunacColors.textMuted,
                        fontSize: 9.5,
                      ),
                    ),
                ],
              ),
            ),
            const SizedBox(width: 6),
            Text(
              '$count 首',
              style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
            ),
            if (trailing != null) ...[const SizedBox(width: 2), trailing],
          ],
        ),
      ),
    );
  }

  /// 主区（v1 `renderLocalMain`）：筛选 + 扫描 + 曲目表。
  Widget _buildListMain() {
    final tracks = _filteredTracks;
    final queue = _activeQueue;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // 队列详情页的标题（v1 队列页标题；媒体库里不画）。
        if (queue != null)
          Padding(
            padding: const EdgeInsets.fromLTRB(12, 8, 10, 0),
            child: Row(
              children: [
                Icon(Icons.queue_music, size: 14, color: LunacColors.accent),
                const SizedBox(width: 6),
                Expanded(
                  child: Text(
                    queue.name.isEmpty ? '未命名队列' : queue.name,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: LunacColors.text,
                      fontSize: 12.5,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                ),
              ],
            ),
          ),
        // 播放列表（v1 `mediaList`）：标题显示文件名 + 曲目数；它不写进左栏。
        if (queue == null && _playlistName != null)
          Padding(
            padding: const EdgeInsets.fromLTRB(12, 8, 10, 0),
            child: Row(
              children: [
                Icon(Icons.playlist_play, size: 14, color: LunacColors.accent),
                const SizedBox(width: 6),
                Expanded(
                  child: Tooltip(
                    message: _playlistPath ?? '',
                    child: Text(
                      _playlistName!,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        color: LunacColors.text,
                        fontSize: 12.5,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                  ),
                ),
                Text(
                  '${_tracks.length} 首（不入库）',
                  style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
                ),
              ],
            ),
          ),
        Padding(
          padding: const EdgeInsets.fromLTRB(12, 8, 10, 4),
          child: Row(
            children: [
              Expanded(child: _buildFilterField()),
              const SizedBox(width: 6),
              if (_activeQueueId == null)
                IconButton(
                  tooltip: '打开播放列表（.m3u / .m3u8 / .pls）',
                  icon: const Icon(Icons.playlist_play, size: 16),
                  color: LunacColors.textDim,
                  visualDensity: VisualDensity.compact,
                  onPressed: _openPlaylist,
                ),
              if (_activeQueueId == null)
                IconButton(
                  tooltip: _queuePickMode ? '退出自定义队列' : '自定义队列',
                  icon: Icon(
                    _queuePickMode
                        ? Icons.playlist_add_check
                        : Icons.playlist_add_check_outlined,
                    size: 16,
                  ),
                  color: _queuePickMode
                      ? LunacColors.accent
                      : LunacColors.textDim,
                  visualDensity: VisualDensity.compact,
                  onPressed: _toggleQueuePickMode,
                ),
              if (_queuePickMode)
                TextButton.icon(
                  onPressed: _pickedPaths.isEmpty
                      ? null
                      : () => _openQueueDialog(seed: [..._pickedPaths]),
                  icon: const Icon(Icons.add, size: 14),
                  label: Text(
                    '添加到队列 (${_pickedPaths.length})',
                    style: const TextStyle(fontSize: 11),
                  ),
                  style: TextButton.styleFrom(
                    foregroundColor: LunacColors.accent,
                    visualDensity: VisualDensity.compact,
                    minimumSize: Size.zero,
                    padding: const EdgeInsets.symmetric(horizontal: 6),
                  ),
                ),
              IconButton(
                // v1 主区那颗「扫描」：后台线程 + 真进度（进度条在左栏轮询）。
                tooltip: _scan.running ? '正在扫描…' : '扫描曲库',
                icon: _scan.running
                    ? const SizedBox(
                        width: 14,
                        height: 14,
                        child: CircularProgressIndicator(strokeWidth: 2),
                      )
                    : const Icon(Icons.refresh, size: 16),
                color: LunacColors.textDim,
                visualDensity: VisualDensity.compact,
                onPressed: _scan.running ? null : _startScan,
              ),
              if (_tracks.isNotEmpty)
                Text(
                  '${tracks.length} 首',
                  style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
                ),
            ],
          ),
        ),
        Expanded(child: _buildTrackList()),
      ],
    );
  }

  /// 筛选框（v1 `.music-local-q`：标题 / 歌手 / 专辑）。
  Widget _buildFilterField() {
    return SizedBox(
      height: 30,
      child: TextField(
        controller: _filter,
        onChanged: (_) => setState(() {}),
        style: TextStyle(color: LunacColors.textDim, fontSize: 12),
        cursorColor: LunacColors.accent,
        decoration: InputDecoration(
          isDense: true,
          hintText: '筛选标题 / 歌手 / 专辑…',
          hintStyle: TextStyle(color: LunacColors.textMuted, fontSize: 11),
          prefixIcon: Icon(
            Icons.search,
            size: 15,
            color: LunacColors.textMuted,
          ),
          prefixIconConstraints: const BoxConstraints(
            minWidth: 30,
            minHeight: 30,
          ),
          filled: true,
          fillColor: LunacColors.background,
          contentPadding: const EdgeInsets.symmetric(vertical: 6),
          border: OutlineInputBorder(
            borderRadius: BorderRadius.circular(7),
            borderSide: BorderSide(color: LunacColors.border),
          ),
          enabledBorder: OutlineInputBorder(
            borderRadius: BorderRadius.circular(7),
            borderSide: BorderSide(color: LunacColors.border),
          ),
          focusedBorder: OutlineInputBorder(
            borderRadius: BorderRadius.circular(7),
            borderSide: BorderSide(color: LunacColors.divider),
          ),
        ),
      ),
    );
  }

  /// 底部播放条（v1 `#music-lnowbar`）：顶部 2px 进度 + 正在播放 + 控制键 + 时间 / 音量。
  Widget _buildNowBar() {
    final track = _currentTrack!;
    final durationMs = _duration?.inMilliseconds ?? 0;
    final maxMs = durationMs > 0 ? durationMs.toDouble() : 1.0;
    final positionMs = (_dragMs ?? _position.inMilliseconds.toDouble()).clamp(
      0.0,
      maxMs,
    );
    final pos = Duration(milliseconds: positionMs.round());
    final dur = Duration(milliseconds: durationMs);
    return Container(
      height: 58,
      decoration: BoxDecoration(
        border: Border(top: BorderSide(color: LunacColors.divider)),
      ),
      child: Stack(
        children: [
          Positioned(
            left: 0,
            right: 0,
            top: 0,
            height: 3,
            child: LayoutBuilder(
              builder: (context, constraints) {
                final fraction = durationMs > 0
                    ? (positionMs / maxMs).clamp(0.0, 1.0)
                    : 0.0;
                return GestureDetector(
                  behavior: HitTestBehavior.opaque,
                  onTapDown: (details) {
                    if (durationMs <= 0 || constraints.maxWidth <= 0) {
                      return;
                    }
                    final ratio =
                        (details.localPosition.dx / constraints.maxWidth).clamp(
                          0.0,
                          1.0,
                        );
                    unawaited(_seekTo((ratio * maxMs).round()));
                  },
                  child: Stack(
                    children: [
                      Positioned.fill(
                        child: Container(color: LunacColors.surfaceHover),
                      ),
                      Align(
                        alignment: Alignment.centerLeft,
                        child: FractionallySizedBox(
                          widthFactor: fraction,
                          heightFactor: 1,
                          child: Container(color: LunacColors.accent),
                        ),
                      ),
                    ],
                  ),
                );
              },
            ),
          ),
          Positioned.fill(
            top: 3,
            child: Padding(
              padding: const EdgeInsets.fromLTRB(12, 4, 10, 4),
              child: Row(
                children: [
                  // 左侧「正在播放」= 进入播放界面（v1 `.music-bnow`）。
                  Expanded(
                    flex: 3,
                    child: Tooltip(
                      message: '打开播放界面',
                      child: InkWell(
                        borderRadius: BorderRadius.circular(6),
                        onTap: () => setState(() => _playerView = true),
                        child: Padding(
                          padding: const EdgeInsets.symmetric(
                            horizontal: 2,
                            vertical: 1,
                          ),
                          child: Row(
                            children: [
                              _buildCover(track, size: 32),
                              const SizedBox(width: 8),
                              Expanded(
                                child: Column(
                                  mainAxisAlignment: MainAxisAlignment.center,
                                  crossAxisAlignment: CrossAxisAlignment.start,
                                  children: [
                                    Text(
                                      track.title,
                                      maxLines: 1,
                                      overflow: TextOverflow.ellipsis,
                                      style: TextStyle(
                                        color: LunacColors.text,
                                        fontSize: 12,
                                      ),
                                    ),
                                    Text(
                                      track.subtitle.isEmpty
                                          ? '未知艺术家'
                                          : track.subtitle,
                                      maxLines: 1,
                                      overflow: TextOverflow.ellipsis,
                                      style: TextStyle(
                                        color: LunacColors.textMuted,
                                        fontSize: 10,
                                      ),
                                    ),
                                  ],
                                ),
                              ),
                              Icon(
                                Icons.open_in_full,
                                size: 12,
                                color: LunacColors.textMuted,
                              ),
                            ],
                          ),
                        ),
                      ),
                    ),
                  ),
                  const SizedBox(width: 8),
                  _buildNowControls(),
                  const SizedBox(width: 4),
                  _buildPlayQueueButton(),
                  const SizedBox(width: 10),
                  Text(
                    '${_fmt(pos)} / ${_fmt(dur)}',
                    style: TextStyle(
                      color: LunacColors.textMuted,
                      fontSize: 10,
                    ),
                  ),
                  const SizedBox(width: 6),
                  Icon(
                    Icons.volume_up_outlined,
                    size: 15,
                    color: LunacColors.textMuted,
                  ),
                  SizedBox(
                    width: 72,
                    child: SliderTheme(
                      data: SliderTheme.of(context).copyWith(
                        trackHeight: 2,
                        thumbShape: const RoundSliderThumbShape(
                          enabledThumbRadius: 4,
                        ),
                        overlayShape: const RoundSliderOverlayShape(
                          overlayRadius: 8,
                        ),
                        activeTrackColor: LunacColors.accent,
                        inactiveTrackColor: LunacColors.surfaceHover,
                        thumbColor: LunacColors.accent,
                      ),
                      child: Slider(
                        value: _volume,
                        onChanged: (value) {
                          setState(() => _volume = value);
                          unawaited(
                            _engine?.setVolume(value) ?? Future<void>.value(),
                          );
                        },
                      ),
                    ),
                  ),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }

  /// 播放队列浮层（v1 `#music-lq-dd`）：列出这一轮在播的那批曲目，点一首立刻播它。
  ///
  /// 与左栏「自定义队列」是两件事：这里只有 `_queue`（点播那一刻的快照）。
  Widget _buildPlayQueueButton() {
    final queue = _queue;
    return MenuAnchor(
      controller: _playQueueMenu,
      // 浮层贴在按钮上方（v1 的 `#music-lq-dd` 也从播放条向上展开）。
      alignmentOffset: const Offset(-236, -300),
      style: MenuStyle(
        backgroundColor: WidgetStatePropertyAll(LunacColors.surface),
        surfaceTintColor: const WidgetStatePropertyAll(Colors.transparent),
        padding: const WidgetStatePropertyAll(EdgeInsets.zero),
        shape: WidgetStatePropertyAll(
          RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(8),
            side: BorderSide(color: LunacColors.border),
          ),
        ),
      ),
      menuChildren: [
        SizedBox(
          width: 276,
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxHeight: 260),
            child: queue.isEmpty
                ? Padding(
                    padding: const EdgeInsets.all(12),
                    child: Text(
                      '播放队列是空的 —— 在本地页点一首歌开始播放',
                      style: TextStyle(
                        color: LunacColors.textMuted,
                        fontSize: 11,
                      ),
                    ),
                  )
                : SingleChildScrollView(
                    primary: false,
                    child: Column(
                      mainAxisSize: MainAxisSize.min,
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        for (var index = 0; index < queue.length; index++)
                          _buildPlayQueueRow(queue[index], index),
                      ],
                    ),
                  ),
          ),
        ),
      ],
      builder: (context, controller, child) => IconButton(
        tooltip: '播放队列',
        icon: const Icon(Icons.list_alt, size: 16),
        color: controller.isOpen ? LunacColors.accent : LunacColors.textMuted,
        visualDensity: VisualDensity.compact,
        constraints: const BoxConstraints.tightFor(width: 26, height: 26),
        padding: EdgeInsets.zero,
        onPressed: () =>
            controller.isOpen ? controller.close() : controller.open(),
      ),
    );
  }

  /// 播放队列浮层里的一行（序号 / 当前标记 + 歌名 + 艺术家）。
  Widget _buildPlayQueueRow(MusicTrack track, int index) {
    final current = track.path == _currentPath;
    return InkWell(
      onTap: () {
        _playQueueMenu.close();
        unawaited(_playTrack(track));
      },
      child: Container(
        color: current ? LunacColors.surfaceHover : null,
        padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
        child: Row(
          children: [
            SizedBox(
              width: 20,
              child: current
                  ? Icon(Icons.volume_up, size: 12, color: LunacColors.accent)
                  : Text(
                      '${index + 1}',
                      textAlign: TextAlign.center,
                      style: TextStyle(
                        color: LunacColors.textMuted,
                        fontSize: 10,
                      ),
                    ),
            ),
            const SizedBox(width: 6),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    track.title,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: current ? LunacColors.accent : LunacColors.text,
                      fontSize: 11.5,
                    ),
                  ),
                  if (track.subtitle.isNotEmpty)
                    Text(
                      track.subtitle,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        color: LunacColors.textMuted,
                        fontSize: 9.5,
                      ),
                    ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }

  /// 播放条里的控制键（v1 本地条：模式 / 上一首 / 播放暂停 / 下一首 / 停止）。
  Widget _buildNowControls() {
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        _roundButton(
          icon: _modeIcon,
          tooltip: _modeTitle,
          color: _mode == _PlayMode.off
              ? LunacColors.textMuted
              : LunacColors.accent,
          onPressed: _cycleMode,
        ),
        const SizedBox(width: 4),
        _roundButton(
          icon: Icons.skip_previous,
          tooltip: '上一首',
          onPressed: () => unawaited(_advance(step: -1, auto: false)),
        ),
        const SizedBox(width: 4),
        _roundButton(
          icon: _playing ? Icons.pause : Icons.play_arrow,
          tooltip: '播放 / 暂停',
          emphasized: true,
          onPressed: () => unawaited(_togglePlay()),
        ),
        const SizedBox(width: 4),
        _roundButton(
          icon: Icons.skip_next,
          tooltip: '下一首',
          onPressed: () => unawaited(_advance(step: 1, auto: false)),
        ),
        const SizedBox(width: 4),
        _roundButton(
          icon: Icons.stop,
          tooltip: '停止播放',
          onPressed: () => unawaited(_stopPlayback()),
        ),
      ],
    );
  }

  /// 播放界面（v1 `#music-v-player`）：封面 + 歌名 + 歌词 + 进度 / 控制。
  Widget _buildPlayerView() {
    if (_currentTrack == null) {
      return const _EmptyHint(
        icon: Icons.queue_music,
        title: '还没有播放任何本地歌曲',
        detail: '返回歌单点一首开始播放',
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(6, 4, 12, 0),
          child: Row(
            children: [
              IconButton(
                tooltip: '返回歌单（Esc）',
                icon: const Icon(Icons.arrow_back, size: 17),
                color: LunacColors.textDim,
                visualDensity: VisualDensity.compact,
                onPressed: () => setState(() => _playerView = false),
              ),
              const SizedBox(width: 2),
              Text(
                '正在播放',
                style: TextStyle(
                  color: LunacColors.text,
                  fontSize: 13,
                  fontWeight: FontWeight.w600,
                ),
              ),
              const Spacer(),
              _buildPlayQueueButton(),
            ],
          ),
        ),
        Expanded(child: _buildPlayer()),
      ],
    );
  }

  /// v1 编辑态：列出 `roots` 表里的目录，逐个「移出列表」。
  Widget _buildRootEditor() {
    final roots = _roots;
    return Container(
      margin: const EdgeInsets.fromLTRB(10, 0, 10, 6),
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
      decoration: BoxDecoration(
        color: LunacColors.background,
        border: Border.all(color: LunacColors.border),
        borderRadius: BorderRadius.circular(6),
      ),
      child: roots.isEmpty
          ? Text(
              '还没有添加目录',
              style: TextStyle(color: LunacColors.textMuted, fontSize: 10.5),
            )
          : ConstrainedBox(
              constraints: const BoxConstraints(maxHeight: 120),
              child: SingleChildScrollView(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    for (final root in roots)
                      Row(
                        children: [
                          Expanded(
                            child: Text(
                              root,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(
                                color: LunacColors.textDim,
                                fontSize: 10.5,
                              ),
                            ),
                          ),
                          Tooltip(
                            message: '移出列表',
                            child: InkWell(
                              onTap: () => _removeRoot(root),
                              borderRadius: BorderRadius.circular(4),
                              child: Padding(
                                padding: EdgeInsets.symmetric(
                                  horizontal: 6,
                                  vertical: 2,
                                ),
                                child: Icon(
                                  Icons.close,
                                  size: 12,
                                  color: LunacColors.textMuted,
                                ),
                              ),
                            ),
                          ),
                        ],
                      ),
                  ],
                ),
              ),
            ),
    );
  }

  Widget _buildTrackList() {
    if (_loading) {
      return const _EmptyHint(
        icon: Icons.hourglass_empty,
        title: '正在扫描本地音乐…',
        detail: '首次打开要读标签，目录大时会慢一点',
      );
    }
    if (_unavailable) {
      return const _EmptyHint(
        icon: Icons.music_off_outlined,
        title: '不可用（rust 桥接未加载）',
      );
    }
    if (_tracks.isEmpty) {
      final roots = _library?.roots ?? const <String>[];
      return Padding(
        padding: const EdgeInsets.fromLTRB(14, 8, 14, 12),
        child: Column(
          mainAxisAlignment: MainAxisAlignment.center,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              roots.isEmpty ? '还没有添加目录' : '目录已添加，但还不在曲库里',
              style: TextStyle(color: LunacColors.textDim, fontSize: 12),
            ),
            const SizedBox(height: 6),
            Text(
              roots.isEmpty
                  ? '点左上角那枚「+」选一个音乐目录（会扫描并建索引）。'
                  : '已添加的目录：\n${roots.join('\n')}\n\n点「开始扫描」把里面的音频读进曲库。',
              style: TextStyle(
                color: LunacColors.textMuted,
                fontSize: 10,
                height: 1.45,
              ),
            ),
            if (_loadError != null) ...[
              const SizedBox(height: 6),
              Text(
                _loadError!,
                style: TextStyle(color: LunacColors.danger, fontSize: 10),
              ),
            ],
            const SizedBox(height: 10),
            Row(
              children: [
                if (roots.isEmpty)
                  TextButton(
                    onPressed: _addRoot,
                    style: TextButton.styleFrom(
                      foregroundColor: LunacColors.accent,
                      backgroundColor: LunacColors.surfaceHover,
                      visualDensity: VisualDensity.compact,
                    ),
                    child: const Text('添加音乐目录', style: TextStyle(fontSize: 12)),
                  )
                else
                  TextButton(
                    onPressed: _scan.running ? null : _startScan,
                    style: TextButton.styleFrom(
                      foregroundColor: LunacColors.accent,
                      backgroundColor: LunacColors.surfaceHover,
                      visualDensity: VisualDensity.compact,
                    ),
                    child: Text(
                      _scan.running ? '扫描中…' : '开始扫描',
                      style: const TextStyle(fontSize: 12),
                    ),
                  ),
              ],
            ),
          ],
        ),
      );
    }
    final tracks = _filteredTracks;
    if (tracks.isEmpty) {
      return const _EmptyHint(
        icon: Icons.filter_alt_off_outlined,
        title: '没有匹配的曲目',
        detail: '换个筛选词再试',
      );
    }
    return ListView.builder(
      controller: _trackScroll,
      padding: const EdgeInsets.only(bottom: 8),
      itemCount: tracks.length,
      itemExtent: 44,
      itemBuilder: (context, index) => _buildTrackRow(tracks[index], index),
    );
  }

  Widget _buildTrackRow(MusicTrack track, int index) {
    final current = track.path == _currentPath;
    final picking = _queuePickMode;
    final picked = _isPicked(track.path);
    // v1：队列详情页在「修改列表」编辑态里逐首移除。
    final removable = _activeQueueId != null && _managingRoots;
    return GestureDetector(
      // v1 右键菜单（`ctxMenuItemsHtml`）：定位到文件 / 删除本地文件。
      onSecondaryTapDown: (details) =>
          _showTrackMenu(track, details.globalPosition),
      child: InkWell(
      onTap: picking ? () => _togglePick(track) : () => _playFromList(track),
      child: Container(
        margin: const EdgeInsets.symmetric(horizontal: 8),
        padding: const EdgeInsets.symmetric(horizontal: 6),
        decoration: BoxDecoration(
          color: picked
              ? LunacColors.accent.withValues(alpha: 0.12)
              : current
              ? LunacColors.surfaceHover
              : null,
          borderRadius: BorderRadius.circular(6),
        ),
        child: Row(
          children: [
            // v1 行首是序号；正在播放的那一行换成跳动的均衡器图标；
            // 勾选态换成复选框（v1 只在 `queueEditOn` / 修改列表编辑态里出现）。
            SizedBox(
              width: 24,
              child: Center(
                child: picking
                    ? Icon(
                        picked
                            ? Icons.check_box
                            : Icons.check_box_outline_blank,
                        size: 14,
                        color: picked
                            ? LunacColors.accent
                            : LunacColors.textMuted,
                      )
                    : current && _playing
                    ? Icon(
                        Icons.graphic_eq,
                        size: 14,
                        color: LunacColors.accent,
                      )
                    : Text(
                        '${index + 1}',
                        style: TextStyle(
                          color: LunacColors.textMuted,
                          fontSize: 10,
                        ),
                      ),
              ),
            ),
            const SizedBox(width: 6),
            _buildCover(track, size: 30),
            const SizedBox(width: 10),
            Expanded(
              child: Column(
                mainAxisAlignment: MainAxisAlignment.center,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    track.title,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: current ? LunacColors.text : LunacColors.textDim,
                      fontSize: 12,
                    ),
                  ),
                  Text(
                    track.subtitle.isEmpty ? '未知艺术家' : track.subtitle,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: LunacColors.textMuted,
                      fontSize: 10,
                    ),
                  ),
                ],
              ),
            ),
            const SizedBox(width: 6),
            Text(
              track.durationMs > 0
                  ? _fmt(Duration(milliseconds: track.durationMs))
                  : '--:--',
              style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
            ),
            if (removable) ...[
              const SizedBox(width: 2),
              IconButton(
                tooltip: '从队列移除',
                icon: const Icon(Icons.close, size: 12),
                color: LunacColors.textMuted,
                visualDensity: VisualDensity.compact,
                constraints: const BoxConstraints.tightFor(
                  width: 22,
                  height: 22,
                ),
                padding: EdgeInsets.zero,
                onPressed: () => _removeTrackFromQueue(track),
              ),
            ],
          ],
        ),
      ),
      ),
    );
  }

  /// 一首歌的右键菜单（v1 `ctxMenuItemsHtml` 的本地曲目那两项）。
  Future<void> _showTrackMenu(MusicTrack track, Offset global) async {
    final overlay = Overlay.of(context).context.findRenderObject();
    if (overlay is! RenderBox) {
      return;
    }
    final choice = await showMenu<String>(
      context: context,
      position: RelativeRect.fromLTRB(
        global.dx,
        global.dy,
        overlay.size.width - global.dx,
        overlay.size.height - global.dy,
      ),
      items: const [
        PopupMenuItem(
          value: 'reveal',
          height: 32,
          child: Text('在资源管理器中显示', style: TextStyle(fontSize: 12)),
        ),
        PopupMenuItem(
          value: 'delete',
          height: 32,
          child: Text('删除本地文件…', style: TextStyle(fontSize: 12)),
        ),
      ],
    );
    if (!mounted) {
      return;
    }
    if (choice == 'reveal') {
      if (!revealMediaPath(track.path)) {
        setState(() => _rootError = '打不开资源管理器');
      }
    } else if (choice == 'delete') {
      await _deleteTrack(track);
    }
  }

  /// 「修改列表」编辑态里，从当前队列移除一首（立即写盘，v1 编辑态保存的等价物）。
  void _removeTrackFromQueue(MusicTrack track) {
    final queues = _queues;
    final activeId = _activeQueueId;
    if (queues == null || activeId == null) {
      return;
    }
    final key = track.path.toLowerCase();
    final error = _saveQueues([
      for (final queue in queues)
        if (queue.id == activeId)
          queue.copyWith(
            paths: [
              for (final path in queue.paths)
                if (path.toLowerCase() != key) path,
            ],
          )
        else
          queue,
    ]);
    if (!mounted) {
      return;
    }
    if (error != null) {
      setState(() => _queueError = error);
      return;
    }
    setState(() => _queueError = null);
    _loadQueues();
  }

  Widget _buildPlayer() {
    final track = _currentTrack;
    if (track == null) {
      return const _EmptyHint(
        icon: Icons.queue_music,
        title: '还没有播放任何本地歌曲',
        detail: '点左侧任意一首开始播放，歌词会按进度滚动',
      );
    }
    return Padding(
      padding: const EdgeInsets.fromLTRB(14, 4, 14, 10),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              _buildCover(track),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      track.title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        color: LunacColors.text,
                        fontSize: 14,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                    const SizedBox(height: 4),
                    Text(
                      track.subtitle.isEmpty ? '未知艺术家' : track.subtitle,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        color: LunacColors.textMuted,
                        fontSize: 11,
                      ),
                    ),
                    if (_playError != null) ...[
                      const SizedBox(height: 4),
                      Text(
                        _playError!,
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          color: LunacColors.danger,
                          fontSize: 10,
                        ),
                      ),
                    ],
                  ],
                ),
              ),
            ],
          ),
          const SizedBox(height: 8),
          Expanded(child: _buildLyrics()),
          if (_notice != null) ...[
            const SizedBox(height: 4),
            Text(
              _notice!,
              style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
            ),
          ],
          _buildProgress(),
          const SizedBox(height: 2),
          _buildControls(),
        ],
      ),
    );
  }

  Widget _buildCover(MusicTrack track, {double size = 100}) {
    final placeholder = Container(
      width: size,
      height: size,
      decoration: BoxDecoration(
        color: LunacColors.surfaceHover,
        border: Border.all(color: LunacColors.border),
        borderRadius: BorderRadius.circular(size >= 60 ? 8 : 5),
      ),
      child: Icon(
        Icons.music_note,
        size: size >= 60 ? 32 : 14,
        color: LunacColors.textMuted,
      ),
    );
    if (track.cover.isEmpty) {
      return placeholder;
    }
    return ClipRRect(
      borderRadius: BorderRadius.circular(size >= 60 ? 8 : 5),
      child: Image.file(
        File(track.cover),
        width: size,
        height: size,
        fit: BoxFit.cover,
        errorBuilder: (context, error, stackTrace) => placeholder,
      ),
    );
  }

  Widget _buildLyrics() {
    final lyrics = _lyrics;
    Widget child;
    if (_lyricsLoading) {
      child = Center(
        child: Text(
          '正在获取歌词…',
          style: TextStyle(color: LunacColors.textMuted, fontSize: 12),
        ),
      );
    } else if (lyrics?.error != null) {
      child = Center(
        child: Text(
          lyrics!.error!,
          textAlign: TextAlign.center,
          style: TextStyle(color: LunacColors.danger, fontSize: 11),
        ),
      );
    } else if (lyrics?.instrumental == true && (lyrics?.isEmpty ?? true)) {
      child = Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(
              '这是一首纯音乐',
              style: TextStyle(color: LunacColors.textMuted, fontSize: 12),
            ),
            const SizedBox(height: 4),
            Text(
              '来源：${lyrics?.sourceLabel ?? ''}',
              style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
            ),
          ],
        ),
      );
    } else if (lyrics == null || lyrics.isEmpty) {
      final plain = lyrics?.plain;
      child = Center(
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 12),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(
                '未找到歌词',
                style: TextStyle(color: LunacColors.textMuted, fontSize: 12),
              ),
              const SizedBox(height: 4),
              Text(
                plain ?? '本地没有同名 .lrc，在线（LRCLIB / 网易云）也没搜到',
                maxLines: plain == null ? 2 : 10,
                overflow: TextOverflow.ellipsis,
                textAlign: TextAlign.center,
                style: TextStyle(
                  color: LunacColors.textMuted,
                  fontSize: 10,
                  height: 1.45,
                ),
              ),
              const SizedBox(height: 6),
              TextButton(
                onPressed: _lyricsLoading ? null : _retryOnlineLyrics,
                style: TextButton.styleFrom(
                  foregroundColor: LunacColors.textDim,
                  padding: const EdgeInsets.symmetric(
                    horizontal: 10,
                    vertical: 2,
                  ),
                  minimumSize: Size.zero,
                  visualDensity: VisualDensity.compact,
                ),
                child: const Text('在线再搜一次', style: TextStyle(fontSize: 10.5)),
              ),
            ],
          ),
        ),
      );
    } else {
      final lines = lyrics.lines;
      child = Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Padding(
            padding: const EdgeInsets.only(left: 2, bottom: 2),
            child: Text(
              lyrics.sourceLabel.isEmpty
                  ? '来源：本地 .lrc'
                  : '来源：${lyrics.sourceLabel}',
              style: TextStyle(color: LunacColors.textMuted, fontSize: 9.5),
            ),
          ),
          Expanded(
            child: NotificationListener<ScrollNotification>(
              onNotification: _onLyricsScroll,
              child: ListView.builder(
                controller: _lyricsScroll,
                padding: const EdgeInsets.symmetric(vertical: 4),
                itemCount: lines.length,
                itemExtent: _lyricLineExtent,
                itemBuilder: (context, index) {
                  final active = index == _activeLine;
                  // v1：已过 + 正在 = 亮，未到 = 暗。
                  final passed = index < _activeLine;
                  return InkWell(
                    onTap: () => _seekTo(lines[index].timeMs),
                    child: Align(
                      alignment: Alignment.centerLeft,
                      child: Text(
                        lines[index].text,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          color: active
                              ? LunacColors.accent
                              : passed
                              ? LunacColors.textDim
                              : LunacColors.textMuted,
                          fontSize: active ? 13 : 12,
                          fontWeight: active ? FontWeight.w600 : FontWeight.w400,
                        ),
                      ),
                    ),
                  );
                },
              ),
            ),
          ),
        ],
      );
    }
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10),
      decoration: BoxDecoration(
        color: LunacColors.background,
        border: Border.all(color: LunacColors.border),
        borderRadius: BorderRadius.circular(8),
      ),
      child: child,
    );
  }

  Widget _buildProgress() {
    final durationMs = _duration?.inMilliseconds ?? 0;
    final maxMs = durationMs > 0 ? durationMs.toDouble() : 1.0;
    final positionMs =
        _dragMs ?? _position.inMilliseconds.toDouble().clamp(0.0, maxMs);
    final pos = Duration(milliseconds: positionMs.round());
    final dur = Duration(milliseconds: durationMs);
    return Row(
      children: [
        Text(
          _fmt(pos),
          style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
        ),
        Expanded(
          child: SliderTheme(
            data: SliderTheme.of(context).copyWith(
              trackHeight: 2,
              thumbShape: const RoundSliderThumbShape(enabledThumbRadius: 5),
              overlayShape: const RoundSliderOverlayShape(overlayRadius: 10),
              activeTrackColor: LunacColors.accent,
              inactiveTrackColor: LunacColors.surfaceHover,
              thumbColor: LunacColors.accent,
            ),
            child: Slider(
              value: durationMs > 0 ? positionMs : 0,
              max: maxMs,
              onChanged: durationMs > 0
                  ? (value) => setState(() => _dragMs = value)
                  : null,
              onChangeEnd: durationMs > 0
                  ? (value) async {
                      setState(() => _dragMs = null);
                      await _seekTo(value.round());
                    }
                  : null,
            ),
          ),
        ),
        Text(
          _fmt(dur),
          style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
        ),
      ],
    );
  }

  Widget _buildControls() {
    return Row(
      children: [
        _roundButton(
          icon: _modeIcon,
          tooltip: _modeTitle,
          color: _mode == _PlayMode.off
              ? LunacColors.textMuted
              : LunacColors.accent,
          onPressed: _cycleMode,
        ),
        const SizedBox(width: 6),
        _roundButton(
          icon: Icons.skip_previous,
          tooltip: '上一首',
          onPressed: () => unawaited(_advance(step: -1, auto: false)),
        ),
        const SizedBox(width: 6),
        _roundButton(
          icon: _playing ? Icons.pause : Icons.play_arrow,
          tooltip: '播放 / 暂停',
          emphasized: true,
          onPressed: () => unawaited(_togglePlay()),
        ),
        const SizedBox(width: 6),
        _roundButton(
          icon: Icons.skip_next,
          tooltip: '下一首',
          onPressed: () => unawaited(_advance(step: 1, auto: false)),
        ),
        const SizedBox(width: 6),
        _roundButton(
          icon: Icons.stop,
          tooltip: '停止播放',
          onPressed: () => unawaited(_stopPlayback()),
        ),
        const Spacer(),
        Icon(Icons.volume_up_outlined, size: 16, color: LunacColors.textMuted),
        SizedBox(
          width: 90,
          child: SliderTheme(
            data: SliderTheme.of(context).copyWith(
              trackHeight: 2,
              thumbShape: const RoundSliderThumbShape(enabledThumbRadius: 4),
              overlayShape: const RoundSliderOverlayShape(overlayRadius: 8),
              activeTrackColor: LunacColors.accent,
              inactiveTrackColor: LunacColors.surfaceHover,
              thumbColor: LunacColors.accent,
            ),
            child: Slider(
              value: _volume,
              onChanged: (value) {
                setState(() => _volume = value);
                unawaited(_engine?.setVolume(value) ?? Future<void>.value());
              },
            ),
          ),
        ),
      ],
    );
  }

  Widget _roundButton({
    required IconData icon,
    required String tooltip,
    required VoidCallback onPressed,
    Color? color,
    bool emphasized = false,
  }) {
    return SizedBox(
      width: emphasized ? 34 : 28,
      height: emphasized ? 34 : 28,
      child: IconButton(
        tooltip: tooltip,
        icon: Icon(icon, size: emphasized ? 20 : 16),
        color: emphasized
            ? LunacColors.background
            : (color ?? LunacColors.textDim),
        style: IconButton.styleFrom(
          padding: EdgeInsets.zero,
          backgroundColor: emphasized
              ? LunacColors.accent
              : LunacColors.surfaceHover,
          shape: const CircleBorder(),
          minimumSize: Size.zero,
          tapTargetSize: MaterialTapTargetSize.shrinkWrap,
        ),
        onPressed: onPressed,
      ),
    );
  }

  String _fmt(Duration duration) {
    final total = duration.inSeconds;
    final minutes = total ~/ 60;
    final seconds = total % 60;
    return '$minutes:${seconds.toString().padLeft(2, '0')}';
  }

  String _playErrorText(Object error) {
    final text = error.toString();
    final lower = text.toLowerCase();
    if (lower.contains('no such file') ||
        lower.contains('not found') ||
        lower.contains('系统找不到')) {
      return '这个文件不在了 —— 可能被移动或删除，重新扫描一次';
    }
    if (lower.contains('decode') ||
        lower.contains('source') ||
        lower.contains('unsupported')) {
      return '这个文件解不开 —— 可能是格式不受支持或文件损坏';
    }
    return '播放失败：$text';
  }
}

/// 列表 / 播放区的空态（图标 + 主文案 + 说明）。
class _EmptyHint extends StatelessWidget {
  const _EmptyHint({required this.icon, required this.title, this.detail});

  final IconData icon;
  final String title;
  final String? detail;

  @override
  Widget build(BuildContext context) {
    return Center(
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 18),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(icon, size: 22, color: LunacColors.textMuted),
            const SizedBox(height: 8),
            Text(
              title,
              textAlign: TextAlign.center,
              style: TextStyle(color: LunacColors.textDim, fontSize: 12),
            ),
            if (detail != null) ...[
              const SizedBox(height: 4),
              Text(
                detail!,
                textAlign: TextAlign.center,
                style: TextStyle(
                  color: LunacColors.textMuted,
                  fontSize: 10,
                  height: 1.45,
                ),
              ),
            ],
          ],
        ),
      ),
    );
  }
}

/// Spotify 在线库弹层（v1 `music.ts` 在线面板的等价入口）。
///
/// 只做「让用户能真的用起来」的那几项：授权 / 断开、播放控制、歌单、搜索、设备接管。
/// **本机播放（librespot）已按用户 2026-11-19 的裁决移除**，不做假开关。
class _SpotifySheet extends StatefulWidget {
  const _SpotifySheet();

  @override
  State<_SpotifySheet> createState() => _SpotifySheetState();
}

class _SpotifySheetState extends State<_SpotifySheet> {
  SpotifyState _state = const SpotifyState();
  final TextEditingController _query = TextEditingController();
  List<Map<String, Object?>> _playlists = const [];
  List<Map<String, Object?>> _devices = const [];
  List<Map<String, Object?>> _results = const [];

  /// 「播放在哪」自动就位的结论（v1 `music_autoconfigure` 的那一句）。
  String _autoNote = '';

  /// 「你的音乐库」当前栏目（artists / albums / shows / top_artists / top_tracks）。
  String _libraryKind = '';

  /// 当前栏目里的条目（歌手 / 专辑 / 播客）。
  List<Map<String, Object?>> _library = const [];

  /// 点开条目后的曲目列（专辑曲目 / 歌手热门曲 / 播客单集）。
  List<Map<String, Object?>> _libraryTracks = const [];
  String _libraryTitle = '';

  Map<String, Object?>? _nowPlaying;
  String? _message;
  String? _playingUri;
  bool _busy = false;
  Timer? _poll;

  /// 「自己的 Client ID / 回调端口」编辑区是否展开（v1 `#music-setup-fields` 的收放）。
  bool _configOpen = false;
  final TextEditingController _clientIdText = TextEditingController();
  final TextEditingController _portText = TextEditingController();

  @override
  void initState() {
    super.initState();
    _refreshState();
    // 编辑框的初值只在开面板时取一次：轮询刷新状态时不能把用户正在输入的内容冲掉。
    _clientIdText.text = _state.builtin ? '' : _state.clientId;
    _portText.text = _state.port == 0 ? '' : '${_state.port}';
    if (_state.connected) {
      _loadPlaylists();
      _loadDevices();
      _loadNowPlaying();
    }
  }

  @override
  void dispose() {
    _poll?.cancel();
    _query.dispose();
    _clientIdText.dispose();
    _portText.dispose();
    super.dispose();
  }

  void _toast(String text) {
    if (mounted) {
      setState(() => _message = text);
    }
  }

  void _refreshState() {
    final state = readMusicSpotifyState();
    if (!mounted) {
      return;
    }
    setState(() => _state = state);
    if (state.connected) {
      _stopPolling();
    } else if (state.pending) {
      _startPolling();
    }
  }

  /// 授权在浏览器里完成：每 1.2s 读一次本地状态（只读配置文件，不联网）。
  void _startPolling() {
    if (_poll != null) {
      return;
    }
    _poll = Timer.periodic(const Duration(milliseconds: 1200), (_) {
      final state = readMusicSpotifyState();
      if (!mounted) {
        return;
      }
      setState(() => _state = state);
      if (state.connected) {
        _stopPolling();
        _toast('已连接 Spotify');
        _loadPlaylists();
        _loadDevices();
        _loadNowPlaying();
      } else if (!state.pending) {
        _stopPolling();
      }
    });
  }

  void _stopPolling() {
    _poll?.cancel();
    _poll = null;
  }

  Future<void> _connect() async {
    if (_busy) {
      return;
    }
    setState(() {
      _busy = true;
      _message = null;
    });
    final reply = await connectMusicSpotify();
    if (!mounted) {
      return;
    }
    setState(() => _busy = false);
    if (!reply.ok) {
      _toast(reply.errorText.isEmpty ? '授权没有启动成功' : reply.errorText);
      _refreshState();
      return;
    }
    _toast('已在浏览器打开授权页；完成授权后这里会自动变成「已连接」');
    _refreshState();
  }

  /// 保存「自己的 Client ID / 回调端口」（v1 `#music-save` 那一颗按钮）。
  ///
  /// 换值会让宿主清掉旧令牌 —— 面板必须把这件事说清楚，否则用户会以为「保存后掉线」是 bug。
  Future<void> _saveConfig() async {
    if (_busy) {
      return;
    }
    final id = _clientIdText.text.trim();
    final port = int.tryParse(_portText.text.trim());
    if (id.isEmpty) {
      _toast('Client ID 不能为空');
      return;
    }
    if (port == null || port < 1024 || port > 65535) {
      _toast('端口要 1024–65535');
      return;
    }
    setState(() {
      _busy = true;
      _message = null;
    });
    final reply = await saveMusicSpotifyConfig(clientId: id, port: port);
    if (!mounted) {
      return;
    }
    setState(() => _busy = false);
    if (!reply.ok) {
      _toast(reply.errorText.isEmpty ? '保存失败' : reply.errorText);
      return;
    }
    _refreshState();
    _toast('已保存；换 Client ID / 端口会清掉旧令牌，需要重新授权');
  }

  Future<void> _disconnect() async {
    if (_busy) {
      return;
    }
    setState(() {
      _busy = true;
      _message = null;
    });
    final reply = await disconnectMusicSpotify();
    if (!mounted) {
      return;
    }
    setState(() {
      _busy = false;
      _playlists = const [];
      _devices = const [];
      _results = const [];
      _library = const [];
      _libraryTracks = const [];
      _libraryKind = '';
      _libraryTitle = '';
      _nowPlaying = null;
    });
    _toast(reply.ok ? '已断开 Spotify' : reply.errorText);
    _refreshState();
  }

  Future<void> _resumeApi() async {
    await callMusicSpotify(const {'action': 'resume'});
    _refreshState();
    _toast('已放开限流闸门');
  }

  /// 统一执行一个 action：失败就把**人话**显示出来（机器码在 `errorText` 里翻好）。
  Future<Object?> _call(Map<String, Object?> payload, {String? busyText}) async {
    if (_busy) {
      return null;
    }
    setState(() {
      _busy = true;
      if (busyText != null) {
        _message = busyText;
      }
    });
    final reply = await callMusicSpotify(payload);
    if (!mounted) {
      return null;
    }
    setState(() => _busy = false);
    if (!reply.ok) {
      _toast(reply.errorText.isEmpty ? '操作没有成功' : reply.errorText);
      if (reply.error.startsWith('ERR_RATE_LIMITED') ||
          reply.error == 'ERR_NOT_CONNECTED') {
        _refreshState();
      }
      return null;
    }
    return reply.data;
  }

  Future<void> _control(String what) async {
    await _call({'action': 'control', 'what': what});
    _loadNowPlaying();
  }

  Future<void> _loadNowPlaying() async {
    final data = await _call(const {'action': 'status'});
    if (!mounted) {
      return;
    }
    setState(() {
      _nowPlaying = data is Map ? Map<String, Object?>.from(data) : null;
    });
  }

  Future<void> _loadPlaylists() async {
    final data = await _call(const {'action': 'playlists'});
    if (!mounted || data is! Map) {
      return;
    }
    final items = data['items'];
    setState(() {
      _playlists = [
        for (final item in (items is List ? items : const []))
          if (item is Map) Map<String, Object?>.from(item),
      ];
    });
  }

  Future<void> _loadDevices() async {
    final data = await _call(const {'action': 'devices'});
    if (!mounted || data is! Map) {
      return;
    }
    final items = data['items'];
    setState(() {
      _devices = [
        for (final item in (items is List ? items : const []))
          if (item is Map) Map<String, Object?>.from(item),
      ];
    });
    // 一台设备都没有时，按 v1 的做法自动就位一次（只会触发一次：结论非空就不再进）。
    if (_devices.isEmpty && _autoNote.isEmpty) {
      await _autoconfigure();
    }
  }

  /// v1 `music_autoconfigure` 的 v2 等价物：把「播放在哪」定下来。
  ///
  /// 本机播放（librespot）已移除 ⇒ 只在现有设备里挑一台；没有设备时如实说明。
  Future<void> _autoconfigure() async {
    final data = await _call(
      const {'action': 'autoconfigure'},
      busyText: '正在选择播放设备…',
    );
    if (!mounted || data is! Map) {
      return;
    }
    final note = data['note'];
    setState(() => _autoNote = note is String ? note : '');
    // 转移之后设备列表的「活跃」标记会变，刷新一次让高亮对上。
    await _loadDevices();
  }

  /// v1 `spotify_set_play_mode`：顺序 / 随机 / 单曲循环。
  ///
  /// Spotify Web API 没有「智能随机」——它只是客户端的本地功能，这里不假装能做。
  Future<void> _setPlayMode(String mode) async {
    final data = await _call({'action': 'play_mode', 'mode': mode});
    if (data == null) {
      return;
    }
    _toast(switch (mode) {
      'shuffle' => '已切到随机播放',
      'repeat_one' => '已切到单曲循环',
      _ => '已切回顺序播放',
    });
  }

  /// 「你的音乐库」五个栏目（v1 `spotify_artists / albums / shows / top_*`）。
  Future<void> _loadLibrary(String kind) async {
    final data = await _call(
      {'action': 'library', 'kind': kind},
      busyText: '载入音乐库…',
    );
    if (!mounted || data is! Map) {
      return;
    }
    final items = data['items'];
    final tracks = data['tracks'];
    setState(() {
      _libraryKind = kind;
      _libraryTitle = _libraryLabel(kind);
      _libraryTracks = const [];
      _library = [
        for (final item in (items is List ? items : const []))
          if (item is Map) Map<String, Object?>.from(item),
      ];
      // 常听歌曲只有曲目，没有「条目」这一层。
      if (kind == 'top_tracks') {
        _libraryTracks = [
          for (final item in (tracks is List ? tracks : const []))
            if (item is Map) Map<String, Object?>.from(item),
        ];
      }
    });
  }

  /// 点开一个库条目 ⇒ 拉它的曲目列（专辑曲目 / 歌手热门曲 / 播客单集）。
  Future<void> _openLibraryItem(Map<String, Object?> item) async {
    final kind = item['kind'];
    final id = item['id'];
    if (kind is! String || id is! String) {
      return;
    }
    final data = await _call(
      {'action': 'item_tracks', 'kind': kind, 'id': id},
      busyText: '载入曲目…',
    );
    if (!mounted || data is! Map) {
      return;
    }
    final items = data['items'];
    setState(() {
      _libraryTitle = '${item['name'] ?? ''}';
      _libraryTracks = [
        for (final track in (items is List ? items : const []))
          if (track is Map) Map<String, Object?>.from(track),
      ];
    });
  }

  String _libraryLabel(String kind) => switch (kind) {
    'artists' => '关注的歌手',
    'albums' => '收藏的专辑',
    'shows' => '收藏的播客',
    'top_artists' => '最常听的歌手',
    'top_tracks' => '最常听的歌曲',
    _ => '音乐库',
  };

  Future<void> _playItem(Map<String, Object?> item) async {
    final uri = item['uri'];
    final context = item['context'];
    if (uri is String && uri.isNotEmpty) {
      await _call({'action': 'play', 'uri': uri});
      setState(() => _playingUri = uri);
    } else if (context is String && context.isNotEmpty) {
      await _call({'action': 'play', 'context': context});
      setState(() => _playingUri = context);
    }
    _loadNowPlaying();
  }

  Future<void> _transfer(String deviceId) async {
    await _call({'action': 'transfer', 'device': deviceId});
    _loadDevices();
  }

  Future<void> _search() async {
    final query = _query.text.trim();
    if (query.isEmpty) {
      _toast('先输入搜索关键词');
      return;
    }
    final data = await _call(
      {'action': 'search', 'q': query},
      busyText: '搜索中…',
    );
    if (!mounted || data is! Map) {
      return;
    }
    final tracks = data['tracks'];
    final playlists = data['playlists'];
    setState(() {
      _results = [
        for (final item in (tracks is List ? tracks : const []))
          if (item is Map) Map<String, Object?>.from(item),
        for (final item in (playlists is List ? playlists : const []))
          if (item is Map)
            <String, Object?>{
              ...Map<String, Object?>.from(item),
              'context': item['uri'],
            },
      ];
    });
  }

  @override
  Widget build(BuildContext context) {
    return Dialog(
      backgroundColor: LunacColors.surface,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(10),
        side: BorderSide(color: LunacColors.border),
      ),
      child: SizedBox(
        width: 520,
        height: 520,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            _buildHeader(),
            Divider(height: 1, color: LunacColors.divider),
            Expanded(
              child: SingleChildScrollView(
                padding: const EdgeInsets.fromLTRB(14, 10, 14, 12),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    _buildAccount(),
                    const SizedBox(height: 10),
                    if (_state.connected) ...[
                      _buildNowPlaying(),
                      const SizedBox(height: 10),
                      _buildDevices(),
                      const SizedBox(height: 10),
                      _buildPlaylists(),
                      const SizedBox(height: 10),
                      _buildLibrary(),
                      const SizedBox(height: 10),
                      _buildSearch(),
                    ],
                    if (_message != null) ...[
                      const SizedBox(height: 10),
                      Text(
                        _message!,
                        style: TextStyle(
                          color: LunacColors.textMuted,
                          fontSize: 10.5,
                          height: 1.45,
                        ),
                      ),
                    ],
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
      padding: const EdgeInsets.fromLTRB(14, 10, 8, 8),
      child: Row(
        children: [
          Text(
            'Spotify 在线库',
            style: TextStyle(
              color: LunacColors.text,
              fontSize: 13,
              fontWeight: FontWeight.w600,
            ),
          ),
          const SizedBox(width: 8),
          Text(
            _state.connected
                ? '已连接${_state.displayName.isEmpty ? '' : '：${_state.displayName}'}'
                : '未连接',
            style: TextStyle(color: LunacColors.textMuted, fontSize: 10.5),
          ),
          const Spacer(),
          IconButton(
            tooltip: '关闭（Esc）',
            icon: const Icon(Icons.close, size: 16),
            color: LunacColors.textDim,
            visualDensity: VisualDensity.compact,
            onPressed: () => Navigator.of(context).pop(),
          ),
        ],
      ),
    );
  }

  Widget _buildAccount() {
    final state = _state;
    if (state.error != null) {
      return Text(
        state.error!,
        style: TextStyle(color: LunacColors.danger, fontSize: 11),
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          '账号授权走 PKCE（浏览器授权 → 本机环回回调），令牌落在 '
          '${state.path.isEmpty ? 'config/music.json' : state.path}。',
          style: TextStyle(
            color: LunacColors.textMuted,
            fontSize: 10,
            height: 1.45,
          ),
        ),
        const SizedBox(height: 6),
        if (state.stopActive) ...[
          Text(
            '${state.stopReason.isEmpty ? 'Spotify 限流' : state.stopReason}：'
            '等 ${state.retryAfterSecs} 秒',
            style: TextStyle(color: LunacColors.danger, fontSize: 10.5),
          ),
          const SizedBox(height: 4),
          Align(
            alignment: Alignment.centerLeft,
            child: _smallButton('立即恢复', _resumeApi),
          ),
          const SizedBox(height: 6),
        ],
        Wrap(
          spacing: 8,
          runSpacing: 6,
          crossAxisAlignment: WrapCrossAlignment.center,
          children: [
            if (!state.connected)
              _smallButton(
                state.pending ? '等待浏览器授权…' : '浏览器授权',
                _connect,
                primary: true,
              ),
            if (state.pending) _smallButton('刷新状态', _refreshState),
            if (state.connected) _smallButton('断开', _disconnect),
            if (!state.connected)
              Text(
                '回调：${state.redirectUri}',
                style: TextStyle(color: LunacColors.textMuted, fontSize: 9.5),
              ),
          ],
        ),
        const SizedBox(height: 6),
        Text(
          state.builtin
              ? '当前使用内置 Client ID（不用填任何东西，直接点「浏览器授权」）。'
              : '当前使用自己的 Client ID：${state.clientId}',
          style: TextStyle(
            color: LunacColors.textMuted,
            fontSize: 9.5,
            height: 1.45,
          ),
        ),
        if (!state.connected) ...[
          const SizedBox(height: 2),
          Align(
            alignment: Alignment.centerLeft,
            child: TextButton(
              onPressed: () => setState(() => _configOpen = !_configOpen),
              style: TextButton.styleFrom(
                foregroundColor: LunacColors.textDim,
                padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 2),
                minimumSize: Size.zero,
                visualDensity: VisualDensity.compact,
              ),
              child: Text(
                _configOpen ? '收起 Client ID 设置' : '使用自己的 Client ID',
                style: const TextStyle(fontSize: 10),
              ),
            ),
          ),
          if (_configOpen) ...[
            const SizedBox(height: 4),
            _configField(
              controller: _clientIdText,
              hint: 'Client ID（developer.spotify.com 创建应用后复制）',
            ),
            const SizedBox(height: 4),
            SizedBox(
              width: 120,
              child: _configField(controller: _portText, hint: '回调端口'),
            ),
            const SizedBox(height: 4),
            Row(
              children: [
                _smallButton('保存', _saveConfig, primary: true),
                const SizedBox(width: 8),
                Expanded(
                  child: Text(
                    'Redirect URI 要填：${state.redirectUri}',
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: LunacColors.textMuted,
                      fontSize: 9.5,
                    ),
                  ),
                ),
              ],
            ),
            const SizedBox(height: 4),
            Text(
              '保存后更换 Client ID / 端口会清掉旧令牌，需要重新授权一次。',
              style: TextStyle(
                color: LunacColors.textMuted,
                fontSize: 9.5,
                height: 1.45,
              ),
            ),
          ],
        ],
      ],
    );
  }

  Widget _buildNowPlaying() {
    final now = _nowPlaying;
    final name = now?['name'];
    final artists = now?['artists'];
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            Text(
              '正在播放',
              style: TextStyle(color: LunacColors.text, fontSize: 11.5),
            ),
            const Spacer(),
            _smallButton('刷新', _loadNowPlaying),
          ],
        ),
        const SizedBox(height: 2),
        Text(
          name is String && name.isNotEmpty
              ? '$name${artists is String && artists.isNotEmpty ? ' · $artists' : ''}'
              : '（Spotify 没有在播放，或没有活动设备）',
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: TextStyle(color: LunacColors.textMuted, fontSize: 10.5),
        ),
        const SizedBox(height: 4),
        Row(
          children: [
            _smallButton('上一首', () => _control('previous')),
            const SizedBox(width: 6),
            _smallButton('播放 / 暂停', () => _control('play')),
            const SizedBox(width: 6),
            _smallButton('下一首', () => _control('next')),
          ],
        ),
        const SizedBox(height: 4),
        Row(
          children: [
            Text(
              '播放模式',
              style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
            ),
            const SizedBox(width: 8),
            _smallButton('顺序', () => _setPlayMode('off')),
            const SizedBox(width: 6),
            _smallButton('随机', () => _setPlayMode('shuffle')),
            const SizedBox(width: 6),
            _smallButton('单曲循环', () => _setPlayMode('repeat_one')),
          ],
        ),
      ],
    );
  }

  Widget _buildDevices() {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            Text(
              '设备',
              style: TextStyle(color: LunacColors.text, fontSize: 11.5),
            ),
            const Spacer(),
            _smallButton('自动就位', _autoconfigure),
            const SizedBox(width: 6),
            _smallButton('刷新设备', _loadDevices),
          ],
        ),
        if (_autoNote.isNotEmpty)
          Text(
            _autoNote,
            style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
          ),
        if (_devices.isEmpty)
          Text(
            '没有设备：先在手机 / 桌面端打开一次 Spotify，再刷新。',
            style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
          )
        else
          for (final device in _devices)
            InkWell(
              onTap: () {
                final id = device['id'];
                if (id is String && id.isNotEmpty) {
                  _transfer(id);
                }
              },
              child: Padding(
                padding: const EdgeInsets.symmetric(vertical: 3),
                child: Row(
                  children: [
                    Icon(
                      device['active'] == true
                          ? Icons.play_circle
                          : Icons.circle_outlined,
                      size: 11,
                      color: device['active'] == true
                          ? LunacColors.accent
                          : LunacColors.textMuted,
                    ),
                    const SizedBox(width: 6),
                    Expanded(
                      child: Text(
                        '${device['name'] ?? ''} · ${device['type'] ?? ''}',
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          color: LunacColors.textDim,
                          fontSize: 10.5,
                        ),
                      ),
                    ),
                    Text(
                      '接管',
                      style: TextStyle(
                        color: LunacColors.textMuted,
                        fontSize: 9.5,
                      ),
                    ),
                  ],
                ),
              ),
            ),
      ],
    );
  }

  Widget _buildPlaylists() {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            Text(
              '我的歌单',
              style: TextStyle(color: LunacColors.text, fontSize: 11.5),
            ),
            const Spacer(),
            _smallButton('载入', _loadPlaylists),
          ],
        ),
        if (_playlists.isEmpty)
          Text(
            '还没有载入歌单',
            style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
          )
        else
          for (final playlist in _playlists)
            InkWell(
              onTap: () => _playItem({'context': playlist['uri']}),
              child: Padding(
                padding: const EdgeInsets.symmetric(vertical: 3),
                child: Row(
                  children: [
                    Icon(
                      playlist['uri'] == _playingUri
                          ? Icons.graphic_eq
                          : Icons.queue_music,
                      size: 12,
                      color: playlist['uri'] == _playingUri
                          ? LunacColors.accent
                          : LunacColors.textMuted,
                    ),
                    const SizedBox(width: 6),
                    Expanded(
                      child: Text(
                        '${playlist['name'] ?? ''}',
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          color: LunacColors.textDim,
                          fontSize: 10.5,
                        ),
                      ),
                    ),
                    Text(
                      '${playlist['tracks'] ?? 0} 首',
                      style: TextStyle(
                        color: LunacColors.textMuted,
                        fontSize: 9.5,
                      ),
                    ),
                  ],
                ),
              ),
            ),
      ],
    );
  }

  Widget _buildLibrary() {
    const kinds = <String, String>{
      'artists': '关注的歌手',
      'albums': '收藏的专辑',
      'shows': '收藏的播客',
      'top_artists': '常听歌手',
      'top_tracks': '常听歌曲',
    };
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            Text(
              '你的音乐库',
              style: TextStyle(color: LunacColors.text, fontSize: 11.5),
            ),
            const Spacer(),
            if (_libraryKind.isNotEmpty)
              _smallButton('刷新', () => _loadLibrary(_libraryKind)),
          ],
        ),
        const SizedBox(height: 2),
        Wrap(
          spacing: 6,
          runSpacing: 4,
          children: [
            for (final entry in kinds.entries)
              _smallButton(
                entry.value,
                () => _loadLibrary(entry.key),
                primary: _libraryKind == entry.key,
              ),
          ],
        ),
        if (_libraryKind.isNotEmpty) ...[
          const SizedBox(height: 4),
          Text(
            _libraryTitle,
            style: TextStyle(color: LunacColors.textMuted, fontSize: 10),
          ),
        ],
        for (final item in _library)
          InkWell(
            onTap: () => _openLibraryItem(item),
            child: Padding(
              padding: const EdgeInsets.symmetric(vertical: 3),
              child: Row(
                children: [
                  Icon(
                    switch (item['kind']) {
                      'artist' => Icons.person_outline,
                      'album' => Icons.album_outlined,
                      _ => Icons.podcasts_outlined,
                    },
                    size: 12,
                    color: LunacColors.textMuted,
                  ),
                  const SizedBox(width: 6),
                  Expanded(
                    child: Text(
                      '${item['name'] ?? ''}'
                      '${item['subtitle'] is String && (item['subtitle'] as String).isNotEmpty ? ' · ${item['subtitle']}' : ''}',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        color: LunacColors.textDim,
                        fontSize: 10.5,
                      ),
                    ),
                  ),
                  if (item['total'] is int && (item['total'] as int) > 0)
                    Text(
                      '${item['total']}',
                      style: TextStyle(
                        color: LunacColors.textMuted,
                        fontSize: 9.5,
                      ),
                    ),
                ],
              ),
            ),
          ),
        for (final track in _libraryTracks)
          InkWell(
            onTap: () => _playItem(track),
            child: Padding(
              padding: const EdgeInsets.symmetric(vertical: 3),
              child: Row(
                children: [
                  Icon(
                    Icons.music_note,
                    size: 12,
                    color: LunacColors.textMuted,
                  ),
                  const SizedBox(width: 6),
                  Expanded(
                    child: Text(
                      '${track['name'] ?? ''}'
                      '${track['artists'] is String && (track['artists'] as String).isNotEmpty ? ' · ${track['artists']}' : ''}',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        color: LunacColors.textDim,
                        fontSize: 10.5,
                      ),
                    ),
                  ),
                ],
              ),
            ),
          ),
        if (_libraryKind.isNotEmpty &&
            _library.isEmpty &&
            _libraryTracks.isEmpty)
          Text(
            '这一栏是空的；若 Spotify 返回 403，说明令牌缺对应权限'
            '（关注歌手要 user-follow-read，常听要 user-top-read）——重新授权一次即可。',
            style: TextStyle(
              color: LunacColors.textMuted,
              fontSize: 9.5,
              height: 1.45,
            ),
          ),
      ],
    );
  }

  Widget _buildSearch() {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            Expanded(
              child: TextField(
                controller: _query,
                onSubmitted: (_) => _search(),
                style: TextStyle(color: LunacColors.text, fontSize: 11),
                decoration: InputDecoration(
                  isDense: true,
                  hintText: '搜索曲目 / 歌单',
                  hintStyle: TextStyle(
                    color: LunacColors.textMuted,
                    fontSize: 10.5,
                  ),
                  filled: true,
                  fillColor: LunacColors.background,
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
            const SizedBox(width: 8),
            _smallButton('搜索', _search, primary: true),
          ],
        ),
        if (_results.isNotEmpty) ...[
          const SizedBox(height: 4),
          for (final item in _results)
            InkWell(
              onTap: () => _playItem(item),
              child: Padding(
                padding: const EdgeInsets.symmetric(vertical: 3),
                child: Row(
                  children: [
                    Icon(
                      item['context'] != null
                          ? Icons.queue_music
                          : Icons.music_note,
                      size: 12,
                      color: LunacColors.textMuted,
                    ),
                    const SizedBox(width: 6),
                    Expanded(
                      child: Text(
                        item['context'] != null
                            ? '${item['name'] ?? ''}（歌单）'
                            : '${item['name'] ?? ''}'
                                  '${item['artists'] is String && (item['artists'] as String).isNotEmpty ? ' · ${item['artists']}' : ''}',
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          color: LunacColors.textDim,
                          fontSize: 10.5,
                        ),
                      ),
                    ),
                  ],
                ),
              ),
            ),
        ],
      ],
    );
  }

  /// 一个小的单行输入框（Client ID / 回调端口；样式与搜索框同一套）。
  Widget _configField({
    required TextEditingController controller,
    required String hint,
  }) {
    return TextField(
      controller: controller,
      style: TextStyle(color: LunacColors.text, fontSize: 11),
      decoration: InputDecoration(
        isDense: true,
        hintText: hint,
        hintStyle: TextStyle(color: LunacColors.textMuted, fontSize: 10),
        filled: true,
        fillColor: LunacColors.background,
        contentPadding: const EdgeInsets.symmetric(horizontal: 8, vertical: 7),
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
    );
  }

  Widget _smallButton(
    String label,
    VoidCallback onTap, {
    bool primary = false,
  }) {
    return TextButton(
      onPressed: _busy ? null : onTap,
      style: TextButton.styleFrom(
        foregroundColor: primary
            ? LunacColors.background
            : LunacColors.textDim,
        backgroundColor: primary
            ? LunacColors.accent
            : LunacColors.surfaceHover,
        padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 3),
        minimumSize: Size.zero,
        visualDensity: VisualDensity.compact,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(999),
        ),
      ),
      child: Text(label, style: const TextStyle(fontSize: 10.5)),
    );
  }
}
