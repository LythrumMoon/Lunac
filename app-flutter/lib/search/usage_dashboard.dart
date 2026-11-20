// 主界面 / 对话窗底部的「用量与成本」表盘（v1 `#token-dashboard` + `#token-usage-panel`）。
//
// 上层：一行紧凑表盘（缓存命中率条 + 百分比 + 本轮累计 tokens），点一下展开面板。
// 面板：**只有数据，没有说明文字**（v1 2026-09-29 用户口径）—— 左上角是价格表日期 /
// 当前统计的模型 / 未定价模型 / 候选价格文件，右上角是「来源」与「时间范围」两个下拉框，
// 中间是逐日（区间）或逐小时（单日）表格，底部有合计行。
//
// 为什么放在对话窗底部：v2 的对话是独立窗（v1 是整个主窗）。表盘要跟着对话实时刷新，
// 放在对话窗的状态栏位置最接近 v1 的 `#status-bar`。
import 'dart:async';

import 'package:flutter/material.dart';

import '../platform/usage_cost.dart';
import '../theme.dart';

/// 会话累计用量（v1 `usageTotals` 的子集）。[revision] 变化时展开中的面板会重读。
class UsageSessionStats {
  const UsageSessionStats({
    this.hit = 0,
    this.miss = 0,
    this.output = 0,
    this.elided = 0,
    this.dropped = 0,
    this.revision = 0,
  });

  /// 命中缓存的输入 token。
  final int hit;

  /// 未命中缓存的输入 token（含缓存写入）。
  final int miss;

  final int output;

  /// 今天压缩省略的工具结果条数 / 丢弃的早期消息条数（解释「命中率为什么掉了」）。
  final int elided;
  final int dropped;

  final int revision;
}

class UsageDashboard extends StatefulWidget {
  const UsageDashboard({super.key, required this.session});

  final UsageSessionStats session;

  @override
  State<UsageDashboard> createState() => _UsageDashboardState();
}

class _UsageDashboardState extends State<UsageDashboard> {
  bool _open = false;
  bool _loading = false;
  UsageCostData? _data;

  UsageRange _range = defaultUsageRange;
  String? _model;
  String? _source;

  @override
  void didUpdateWidget(UsageDashboard oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (_open && oldWidget.session.revision != widget.session.revision) {
      _reload();
    }
  }

  void _toggle() {
    setState(() => _open = !_open);
    if (_open) {
      _reload();
    }
  }

  void _reload() {
    setState(() => _loading = true);
    // 读盘（近 30 个 JSONL）不放在 build 里，避免展开的一帧卡住。
    unawaited(
      Future(() {
        final data = loadUsageCost();
        if (!mounted) {
          return;
        }
        setState(() {
          _data = data;
          _loading = false;
        });
      }),
    );
  }

  @override
  Widget build(BuildContext context) {
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (_open) _buildPanel(),
        _buildBar(),
      ],
    );
  }

  /// 紧凑表盘：命中率条 + 百分比 + 累计 tokens（v1 `updateTokenDashboard`）。
  Widget _buildBar() {
    final hit = widget.session.hit;
    final miss = widget.session.miss;
    final input = hit + miss;
    final total = input + widget.session.output;
    final hitPct = input > 0 ? (hit / input * 100).round() : 0;
    final compact = widget.session.elided + widget.session.dropped > 0
        ? ' · 压缩：省略 ${widget.session.elided} 个工具结果、丢弃 ${widget.session.dropped} 条早期消息'
        : '';
    return InkWell(
      onTap: _toggle,
      child: SizedBox(
        height: 20,
        child: Row(
          children: [
            Tooltip(
              message:
                  '本轮累计 · 命中 ${fmtDashboardTokens(hit)} / 未命中 ${fmtDashboardTokens(miss)} / 合计 ${fmtDashboardTokens(total)}$compact',
              child: ClipRRect(
                borderRadius: BorderRadius.circular(3),
                child: SizedBox(
                  width: 66,
                  height: 6,
                  child: Row(
                    children: [
                      if (input > 0) ...[
                        Expanded(
                          flex: hit,
                          child: ColoredBox(color: LunacColors.accent),
                        ),
                        Expanded(
                          flex: miss,
                          child: ColoredBox(
                            color: LunacColors.textMuted.withValues(alpha: 0.45),
                          ),
                        ),
                      ] else
                        ColoredBox(
                          color: LunacColors.surfaceHover,
                          child: const SizedBox.expand(),
                        ),
                    ],
                  ),
                ),
              ),
            ),
            const SizedBox(width: 6),
            Text(
              '$hitPct%',
              style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
            ),
            const SizedBox(width: 8),
            Text(
              fmtDashboardTokens(total),
              style: TextStyle(color: LunacColors.textDim, fontSize: 11),
            ),
            const Spacer(),
            Icon(
              _open ? Icons.keyboard_arrow_down : Icons.keyboard_arrow_up,
              size: 14,
              color: LunacColors.textMuted,
            ),
            Text(
              '用量与成本',
              style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
            ),
          ],
        ),
      ),
    );
  }

  /// 展开面板：数据头 + 两个下拉框 + 表格（含合计行）。
  Widget _buildPanel() {
    final data = _data;
    return Container(
      margin: const EdgeInsets.only(bottom: 4),
      padding: const EdgeInsets.fromLTRB(8, 6, 8, 6),
      decoration: BoxDecoration(
        color: LunacColors.surface,
        border: Border.all(color: LunacColors.border),
        borderRadius: BorderRadius.circular(8),
      ),
      child: _loading || data == null
          ? SizedBox(
              height: 60,
              child: Center(
                child: SizedBox(
                  width: 14,
                  height: 14,
                  child: CircularProgressIndicator(
                    strokeWidth: 1.6,
                    color: LunacColors.accent,
                  ),
                ),
              ),
            )
          : _buildData(data),
    );
  }

  Widget _buildData(UsageCostData data) {
    final scoped = scopedUsageData(
      data,
      _range,
      model: _model,
      source: _source,
    );
    final totals = sumUsageCost(scoped);
    final rows = costRows(scoped, _range);
    final models = usageModels(data);
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            _kv('价格表', data.hasPrices ? (data.pricing?.updatedAt ?? '—') : '—'),
            const SizedBox(width: 10),
            if (models.isEmpty)
              _kv('模型', '—')
            else if (models.length == 1)
              _kv('模型', modelLabel(models.single))
            else
              _kvWidget(
                '模型',
                _Dropdown<String?>(
                  value: _model,
                  items: [
                    (value: null, label: '总计'),
                    for (final model in models)
                      (value: model, label: modelLabel(model)),
                  ],
                  onChanged: (value) => setState(() => _model = value),
                ),
              ),
            if (totals.unpriced.isNotEmpty) ...[
              const SizedBox(width: 10),
              Flexible(
                child: _kv('未定价', unpricedLabels(totals.unpriced).join(', ')),
              ),
            ],
            if (data.pending != null) ...[
              const SizedBox(width: 10),
              Flexible(
                child: _kv(
                  '候选价格',
                  data.state?.pendingPath.isNotEmpty == true
                      ? data.state!.pendingPath
                      : '—',
                ),
              ),
            ],
            const Spacer(),
            _Dropdown<String>(
              value: _source ?? 'all',
              items: const [
                (value: 'all', label: '全部'),
                (value: 'pet', label: '桌宠'),
                (value: 'chat', label: '普通聊天'),
                (value: 'cron', label: '定时任务'),
              ],
              onChanged: (value) => setState(
                () => _source = value == null || value == 'all' ? null : value,
              ),
            ),
            const SizedBox(width: 8),
            _Dropdown<String>(
              value: _range.value,
              items: [
                (value: 'today', label: '今天'),
                (value: 'days:7', label: '近 7 天'),
                (value: 'days:30', label: '近 30 天'),
                for (final day in data.days.reversed)
                  (value: 'date:${day.date}', label: day.date),
              ],
              onChanged: (value) {
                if (value != null) {
                  setState(() => _range = UsageRange.parse(value));
                }
              },
            ),
          ],
        ),
        const SizedBox(height: 4),
        if (rows.isEmpty)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 10),
            child: Text(
              '该范围内没有用量',
              style: TextStyle(color: LunacColors.textMuted, fontSize: 11),
            ),
          )
        else
          SingleChildScrollView(
            scrollDirection: Axis.horizontal,
            child: _buildTable(data, rows, totals),
          ),
      ],
    );
  }

  Widget _buildTable(
    UsageCostData data,
    List<CostRow> rows,
    UsageTotals totals,
  ) {
    const widths = [86.0, 44.0, 62.0, 62.0, 56.0, 62.0, 76.0, 58.0, 54.0, 46.0];
    const headers = [
      '时间',
      '请求',
      '输入·命中',
      '输入·未命中',
      '缓存写入',
      '输出',
      '金额',
      '未命中/请求',
      '输出/请求',
      '占比',
    ];
    final hasPrices = data.hasPrices;
    Widget cell(
      String text,
      int index, {
      Color? color,
      bool header = false,
      bool bold = false,
    }) {
      return SizedBox(
        width: widths[index],
        child: Padding(
          padding: const EdgeInsets.symmetric(vertical: 2, horizontal: 2),
          child: Text(
            text,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(
              color: color ??
                  (header
                      ? LunacColors.textMuted
                      : LunacColors.textDim),
              fontSize: 10.5,
              fontWeight: bold ? FontWeight.w600 : FontWeight.w400,
            ),
          ),
        ),
      );
    }

    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(children: [
          for (var i = 0; i < headers.length; i++) cell(headers[i], i, header: true),
        ]),
        Divider(height: 5, thickness: 0.5, color: LunacColors.border),
        for (final row in rows)
          Row(children: [
            cell(row.label, 0),
            cell('${row.requests}', 1),
            cell(fmtTokenCount(row.hit), 2),
            cell(fmtTokenCount(row.miss), 3),
            cell(fmtTokenCount(row.write), 4),
            cell(fmtTokenCount(row.output), 5),
            cell(
              amountText(hasPrices, row.amount, row.exact),
              6,
              color: hasPrices ? LunacColors.text : LunacColors.textMuted,
            ),
            cell(perRequestText(row.miss, row.requests), 7),
            cell(perRequestText(row.output, row.requests), 8),
            cell(shareText(hasPrices, row.amount, totals.amount), 9),
          ]),
        const Divider(height: 5, thickness: 0.5),
        Row(children: [
          cell('合计', 0, bold: true, color: LunacColors.text),
          cell('${totals.requests}', 1, bold: true, color: LunacColors.text),
          cell(fmtTokenCount(totals.hit), 2, bold: true, color: LunacColors.text),
          cell(fmtTokenCount(totals.miss), 3, bold: true, color: LunacColors.text),
          cell(fmtTokenCount(totals.write), 4, bold: true, color: LunacColors.text),
          cell(fmtTokenCount(totals.output), 5, bold: true, color: LunacColors.text),
          cell(
            amountText(hasPrices, totals.amount, totals.exact),
            6,
            bold: true,
            color: LunacColors.text,
          ),
          cell(perRequestText(totals.miss, totals.requests), 7),
          cell(perRequestText(totals.output, totals.requests), 8),
          cell(shareText(hasPrices, totals.amount, totals.amount), 9),
        ]),
      ],
    );
  }

  Widget _kv(String key, String value) => _kvWidget(
    key,
    Text(
      value,
      maxLines: 1,
      overflow: TextOverflow.ellipsis,
      style: TextStyle(color: LunacColors.textDim, fontSize: 10.5),
    ),
  );

  Widget _kvWidget(String key, Widget value) {
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        Text(
          key,
          style: TextStyle(color: LunacColors.textMuted, fontSize: 10.5),
        ),
        const SizedBox(width: 4),
        value,
      ],
    );
  }
}

/// 紧凑下拉框（透明底、无下划线、深色菜单）。
class _Dropdown<T> extends StatelessWidget {
  const _Dropdown({
    required this.value,
    required this.items,
    required this.onChanged,
  });

  final T value;
  final List<({T value, String label})> items;
  final ValueChanged<T?> onChanged;

  @override
  Widget build(BuildContext context) {
    return DropdownButtonHideUnderline(
      child: DropdownButton<T>(
        value: value,
        isDense: true,
        dropdownColor: LunacColors.surface,
        style: TextStyle(color: LunacColors.textDim, fontSize: 10.5),
        iconSize: 14,
        iconEnabledColor: LunacColors.textMuted,
        items: [
          for (final item in items)
            DropdownMenuItem(value: item.value, child: Text(item.label)),
        ],
        onChanged: onChanged,
      ),
    );
  }
}
