// OCR 面板（v1 `ocr` 插件的原生等价物）。
//
// 版式对齐 v1 `app/src/plugins/builtin/ocr.ts` 的 `buildDetachedPanelHtml()`：
//   左半 50%：图片预览（空态「暂无图片」）+ 底部三按钮「截屏识别 / 识别剪贴板 / 选择文件」；
//   右半 50%：状态行（默认「PP-OCRv4 · 离线高精度 · 中/英/日/韩/俄」）+ 右上「复制结果」
//            + 文本区（占位「识别结果显示在这里…」）。
// 文案全部取 v1 `i18n.ts` 的 `ocr.*` 原文；识别在 Rust 子进程里做，本面板只展示。
//
// 与 v1 的差异（如实登记）：
//   · 「截屏识别」v1 是宿主自绘的覆盖窗（`src-tauri/src/screenshot.rs`）；v2 还没有那扇窗，
//     改拉起系统截图浮层（`ms-screenclip:`）再取剪贴板里的新图，后续链路同 v1；
//   · 未装引擎时 v1 在状态行内联「下载并安装」；v2 的插件市场还没有 archive 依赖安装，
//     这里只给落点提示（不假装能下）。
import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../platform/clipboard_payload.dart';
import '../platform/ocr.dart';
import '../theme.dart';

/// 图片扩展名（与 v1 剪贴板图片判定同一组）。
const Set<String> _imageExtensions = {
  'png',
  'jpg',
  'jpeg',
  'bmp',
  'gif',
  'webp',
  'tif',
  'tiff',
};

/// v1 `ocr.*` 文案（zh-CN 原文）。
const String _noImageText = '暂无图片';
const String _loadFailedText = '图片加载失败';
const String _defaultStatus = 'PP-OCRv4 · 离线高精度 · 中/英/日/韩/俄';
const String _readingText = '读取剪贴板...';
const String _noClipboardText = '剪贴板中暂无图片，请复制图片后重试';
const String _recognizingText = '识别中 (PaddleOCR)...';
const String _doneText = '完成 (PP-OCRv4)';
const String _failedPrefix = '失败: ';
const String _loadingImageText = '加载图片...';
const String _shotCapturingText = '正在截屏…';
const String _shotFailedPrefix = '截屏失败：';
const String _noText = '（未识别到文字）';
const String _engineMissingText = '未安装 OCR 引擎';
const String _placeholderText = '识别结果显示在这里…';

/// 依据扩展名判断是不是图片路径（不碰文件系统）。
bool isOcrImagePath(String path) {
  final dot = path.lastIndexOf('.');
  if (dot < 0 || dot + 1 >= path.length) {
    return false;
  }
  return _imageExtensions.contains(path.substring(dot + 1).toLowerCase());
}

/// OCR 面板：搜索「OCR / 识别 / 图片转文字」进入。
class OcrPanel extends StatefulWidget {
  const OcrPanel({
    super.key,
    required this.onClose,
    this.attachments = const [],
    this.windowControls,
  });

  /// 关闭面板（Esc / 返回按钮）。
  final VoidCallback onClose;

  /// 独立窗的「置顶 / 最小化 / 关闭」（由 `plugin_window_main` 注入）。
  /// 非 null ⇒ 内嵌模式不再画「返回」，三个窗口键常驻右上角。
  final Widget? windowControls;

  /// 当前搜索栏附件（只取第一张图片，与 v1 单图识别同口径）。
  final List<String> attachments;

  @override
  State<OcrPanel> createState() => _OcrPanelState();
}

class _OcrPanelState extends State<OcrPanel> {
  late final FocusNode _panelFocus = FocusNode(
    debugLabel: 'ocr-panel',
    onKeyEvent: _onKey,
  );

  final TextEditingController _resultCtrl = TextEditingController();

  /// 引擎状态；null = rust 桥接不可用。
  OcrStatus? _status;

  /// 当前预览 / 识别的图片。
  String? _imagePath;

  /// v1 的 `#ocr-status-line`。
  String _statusText = _defaultStatus;

  bool _busy = false;
  bool _copied = false;
  Timer? _copiedTimer;

  @override
  void initState() {
    super.initState();
    _status = readOcrStatus();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) {
        return;
      }
      _panelFocus.requestFocus();
      final first = _firstImageAttachment(widget.attachments);
      if (first != null) {
        // v1 带预置图片进面板时直接识别（`__lunac_ocr_image` 分支）。
        unawaited(_recognizeFile(first));
      }
    });
  }

  @override
  void dispose() {
    _copiedTimer?.cancel();
    _resultCtrl.dispose();
    _panelFocus.dispose();
    super.dispose();
  }

  static String? _firstImageAttachment(List<String> attachments) {
    for (final path in attachments) {
      if (isOcrImagePath(path)) {
        return path;
      }
    }
    return null;
  }

  KeyEventResult _onKey(FocusNode node, KeyEvent event) {
    if (event is KeyDownEvent &&
        event.logicalKey == LogicalKeyboardKey.escape) {
      widget.onClose();
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  bool get _engineReady => _status?.installed == true;

  void _setStatus(String text) {
    if (mounted) {
      setState(() => _statusText = text);
    }
  }

  /// v1 `ensureEngineReady`：未装引擎时把状态行换成安装提示。
  bool _ensureEngineReady() {
    if (_engineReady) {
      return true;
    }
    _setStatus(_engineMissingText);
    return false;
  }

  /// 识别一张图（v1 `ocrImageFile` / `ocrFromDataUrl` 的公共后续）。
  Future<void> _recognizeFile(String path) async {
    if (_busy) {
      return;
    }
    setState(() {
      _imagePath = path;
      _statusText = _loadingImageText;
    });
    if (!_ensureEngineReady()) {
      return;
    }
    await _runRecognize();
  }

  Future<void> _runRecognize() async {
    final path = _imagePath;
    if (path == null) {
      return;
    }
    setState(() {
      _busy = true;
      _statusText = _recognizingText;
    });
    final reply = await recognizeImage(path);
    if (!mounted) {
      return;
    }
    setState(() {
      _busy = false;
      if (reply.isError) {
        _statusText = '$_failedPrefix${reply.error}';
        return;
      }
      _statusText = _doneText;
      final text = reply.result!.text.trim();
      _resultCtrl.text = text.isEmpty ? _noText : text;
    });
  }

  /// 「识别剪贴板」（v1 `ocr-clipboard-btn`）。
  Future<void> _recognizeClipboard() async {
    if (_busy) {
      return;
    }
    _setStatus(_readingText);
    _resultCtrl.clear();
    final payload = readClipboardPayload();
    final picked = <String>[];
    if (payload.kind == ClipboardPayloadKind.image) {
      picked.addAll(payload.paths);
    } else if (payload.kind == ClipboardPayloadKind.files) {
      picked.addAll(payload.paths.where(isOcrImagePath));
    }
    final path = picked.isEmpty ? null : picked.first;
    if (path == null) {
      _setStatus(_noClipboardText);
      return;
    }
    await _recognizeFile(path);
  }

  /// 「截屏识别」（v1 `ocr-screen-btn`：先截、再识别）。
  Future<void> _captureScreen() async {
    if (_busy) {
      return;
    }
    _setStatus(_shotCapturingText);
    final capture = await captureScreenForOcr();
    if (!mounted) {
      return;
    }
    if (capture.isError) {
      _setStatus('$_shotFailedPrefix${capture.error}');
      return;
    }
    await _recognizeFile(capture.path!);
  }

  /// 「选择文件」（v1 `ocr-file-btn`）。
  Future<void> _pickFile() async {
    if (_busy) {
      return;
    }
    final path = pickOcrImage();
    if (path.isEmpty) {
      return; // 取消：状态行不变（v1 同）。
    }
    await _recognizeFile(path);
  }

  /// 「复制结果」（v1 `ocr-copy-btn`）。
  Future<void> _copyResult() async {
    try {
      await Clipboard.setData(ClipboardData(text: _resultCtrl.text));
    } catch (_) {
      return; // 无平台通道：不假装复制成功。
    }
    if (!mounted) {
      return;
    }
    setState(() => _copied = true);
    _copiedTimer?.cancel();
    _copiedTimer = Timer(const Duration(milliseconds: 1500), () {
      if (mounted) {
        setState(() => _copied = false);
      }
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
        child: Column(
          children: [
            _buildHeader(),
            Expanded(child: _buildTwoColumnBody()),
          ],
        ),
      ),
    );
  }

  Widget _buildHeader() {
    return Padding(
      padding: const EdgeInsets.fromLTRB(6, 6, 12, 4),
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
            'OCR 文字识别',
            style: TextStyle(
              color: LunacColors.text,
              fontSize: 14,
              fontWeight: FontWeight.w600,
            ),
          ),
          if (widget.windowControls != null) ...[
            const Spacer(),
            widget.windowControls!,
          ],
        ],
      ),
    );
  }

  /// v1 `.ocr-detached-layout`：左右各 50%，中间一条分隔线。
  Widget _buildTwoColumnBody() {
    return Row(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Expanded(child: _buildImagePanel()),
        Container(width: 1, color: LunacColors.border),
        Expanded(child: _buildTextPanel()),
      ],
    );
  }

  Widget _buildImagePanel() {
    return Column(
      children: [
        Expanded(child: _buildPreview()),
        Container(height: 1, color: LunacColors.border),
        Padding(
          padding: const EdgeInsets.fromLTRB(12, 10, 12, 10),
          child: Row(
            children: [
              Expanded(
                child: _actionButton('截屏识别', onPressed: _captureScreen),
              ),
              const SizedBox(width: 6),
              Expanded(
                child: _actionButton(
                  '识别剪贴板',
                  primary: true,
                  onPressed: _recognizeClipboard,
                ),
              ),
              const SizedBox(width: 6),
              Expanded(
                child: _actionButton('选择文件', onPressed: _pickFile),
              ),
            ],
          ),
        ),
      ],
    );
  }

  Widget _buildPreview() {
    final path = _imagePath;
    if (path == null) {
      return Center(
        child: Text(
          _noImageText,
          style: TextStyle(color: LunacColors.textMuted, fontSize: 13),
        ),
      );
    }
    return Padding(
      padding: const EdgeInsets.all(12),
      child: Center(
        child: Image.file(
          File(path),
          fit: BoxFit.contain,
          errorBuilder: (context, error, stackTrace) => Text(
            _loadFailedText,
            style: TextStyle(color: LunacColors.textMuted, fontSize: 13),
          ),
        ),
      ),
    );
  }

  Widget _actionButton(
    String label, {
    required VoidCallback onPressed,
    bool primary = false,
  }) {
    return SizedBox(
      height: 30,
      child: TextButton(
        onPressed: onPressed,
        style: TextButton.styleFrom(
          foregroundColor: primary ? LunacColors.text : LunacColors.textDim,
          backgroundColor: primary
              ? LunacColors.surfaceHover
              : LunacColors.background,
          side: BorderSide(
            color: primary ? LunacColors.divider : LunacColors.border,
          ),
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(5),
          ),
          padding: const EdgeInsets.symmetric(horizontal: 8),
          minimumSize: Size.zero,
          visualDensity: VisualDensity.compact,
        ),
        child: Text(
          label,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: const TextStyle(fontSize: 11),
        ),
      ),
    );
  }

  /// v1 `.ocr-text-panel`：状态行在顶、复制按钮绝对定位右上、文本区占满其余。
  Widget _buildTextPanel() {
    // 桥接缺失时状态行如实说明（单测环境没有 lunac_core.dll）。
    final statusText = _status == null ? '不可用（rust 桥接未加载）' : _statusText;
    return Stack(
      children: [
        Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(12, 8, 96, 4),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    statusText,
                    style: TextStyle(
                      color: LunacColors.textDim,
                      fontSize: 11,
                      height: 1.3,
                    ),
                  ),
                  if (!_engineReady && _status != null) ...[
                    const SizedBox(height: 2),
                    Text(
                      _engineHint(_status!),
                      style: TextStyle(
                        color: LunacColors.textMuted,
                        fontSize: 10,
                        height: 1.4,
                      ),
                    ),
                  ],
                ],
              ),
            ),
            Expanded(
              child: Padding(
                padding: const EdgeInsets.fromLTRB(12, 0, 12, 12),
                child: TextField(
                  controller: _resultCtrl,
                  expands: true,
                  maxLines: null,
                  minLines: null,
                  textAlignVertical: TextAlignVertical.top,
                  style: TextStyle(
                    color: LunacColors.text,
                    fontSize: 13,
                    height: 1.7,
                  ),
                  decoration: InputDecoration(
                    hintText: _placeholderText,
                    hintStyle: TextStyle(
                      color: LunacColors.textMuted,
                      fontSize: 13,
                    ),
                    border: InputBorder.none,
                    isCollapsed: true,
                    contentPadding: EdgeInsets.zero,
                  ),
                ),
              ),
            ),
          ],
        ),
        Positioned(
          top: 8,
          right: 10,
          child: SizedBox(
            height: 24,
            child: TextButton(
              onPressed: _copyResult,
              style: TextButton.styleFrom(
                foregroundColor: LunacColors.textDim,
                backgroundColor: LunacColors.background,
                side: BorderSide(color: LunacColors.border),
                shape: RoundedRectangleBorder(
                  borderRadius: BorderRadius.circular(4),
                ),
                padding: const EdgeInsets.symmetric(horizontal: 10),
                minimumSize: Size.zero,
                visualDensity: VisualDensity.compact,
              ),
              child: Text(
                _copied ? '已复制' : '复制结果',
                style: const TextStyle(fontSize: 10.5),
              ),
            ),
          ),
        ),
      ],
    );
  }

  /// 未装引擎时的落点提示（v1 是「下载并安装」按钮，v2 暂时只给路径）。
  static String _engineHint(OcrStatus status) {
    if (status.error != null) {
      return status.error!;
    }
    final target = status.searchPaths.isNotEmpty
        ? status.searchPaths.first
        : r'<exe 根>\Modules\ocr\paddle-ocr';
    return '把 PaddleOCR-json 解压到 $target（目录里要有 PaddleOCR-json.exe 与 models 子目录）。';
  }
}
