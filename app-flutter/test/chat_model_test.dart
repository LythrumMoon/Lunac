// M2 对话模型（stream-json 折叠）的纯 Dart 测试：不碰 FFI。
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/platform/chat_history.dart';
import 'package:lunac/search/chat_model.dart';

ChatEntry lastOf(ChatModel model, ChatEntryKind kind) =>
    model.entries.lastWhere((entry) => entry.kind == kind);

void main() {
  test(
    'stream deltas append and the whole assistant message replaces them',
    () {
      final model = ChatModel()..addUser('你好');
      expect(model.busy, isTrue);

      model.apply({
        'type': 'stream_event',
        'event': {
          'type': 'content_block_delta',
          'delta': {'type': 'text_delta', 'text': '你'},
        },
      });
      model.apply({
        'type': 'stream_event',
        'event': {
          'type': 'content_block_delta',
          'delta': {'type': 'text_delta', 'text': '好'},
        },
      });
      expect(lastOf(model, ChatEntryKind.assistant).text, '你好');

      // 整包到达：覆盖增量，不产生第二条助手消息。
      model.apply({
        'type': 'assistant',
        'message': {
          'role': 'assistant',
          'content': [
            {'type': 'text', 'text': '你好呀'},
          ],
        },
      });
      final assistants = model.entries
          .where((entry) => entry.kind == ChatEntryKind.assistant)
          .toList();
      expect(assistants.length, 1);
      expect(assistants.single.text, '你好呀');

      model.apply({'type': 'result', 'subtype': 'success', 'is_error': false});
      expect(model.busy, isFalse);
    },
  );

  // core-agent 的真实顺序是：增量 → message_stop → 整包 assistant。
  // 2026-11-15 前 `message_stop` 会清掉「正在累积的条目」，整包于是再建一条，
  // 界面上同一段小输出出现两次。这条用例钉住「只出现一次」。
  test('message_stop before the whole message does not duplicate it', () {
    final model = ChatModel()..addUser('q');
    model.apply({
      'type': 'stream_event',
      'event': {
        'type': 'content_block_delta',
        'delta': {'type': 'text_delta', 'text': '两件事都改完了'},
      },
    });
    model.apply({
      'type': 'stream_event',
      'event': {'type': 'message_stop'},
    });
    model.apply({
      'type': 'assistant',
      'message': {
        'role': 'assistant',
        'content': [
          {'type': 'text', 'text': '两件事都改完了'},
        ],
      },
    });

    final assistants = model.entries
        .where((entry) => entry.kind == ChatEntryKind.assistant)
        .toList();
    expect(assistants.length, 1);
    expect(assistants.single.text, '两件事都改完了');
    expect(
      model.messages.where((message) => message.role == 'assistant').length,
      1,
    );
  });

  // 下一轮的小结仍要单独成条：收口发生在「合并完整包」之后，不是 message_stop。
  test('the next round starts a new entry after a tool round', () {
    final model = ChatModel()..addUser('q');
    model.apply({
      'type': 'assistant',
      'message': {
        'content': [
          {'type': 'text', 'text': '先量一下'},
          {
            'type': 'tool_use',
            'id': 't1',
            'name': 'Cmd',
            'input': {'command': 'dir'},
          },
        ],
      },
    });
    model.apply({
      'type': 'stream_event',
      'event': {
        'type': 'content_block_delta',
        'delta': {'type': 'text_delta', 'text': '量完了，接着改'},
      },
    });

    final texts = model.entries
        .where((entry) => entry.kind == ChatEntryKind.assistant)
        .map((entry) => entry.text)
        .toList();
    expect(texts, ['先量一下', '量完了，接着改']);
  });

  test('thinking deltas land in a dim entry of their own', () {
    final model = ChatModel()..addUser('q');
    model.apply({
      'type': 'stream_event',
      'event': {
        'type': 'content_block_delta',
        'delta': {'type': 'thinking_delta', 'thinking': '先想一下'},
      },
    });
    expect(lastOf(model, ChatEntryKind.thinking).text, '先想一下');
  });

  test(
    'long streaming thinking keeps its beginning (no 4000-char head loss)',
    () {
      final model = ChatModel()..addUser('q');
      final chunk = List.filled(100, '思考内容').join(); // 每次 400 字
      for (var i = 0; i < 30; i++) {
        model.apply({
          'type': 'stream_event',
          'event': {
            'type': 'content_block_delta',
            'delta': {'type': 'thinking_delta', 'thinking': chunk},
          },
        });
      }
      final thinking = lastOf(model, ChatEntryKind.thinking).text;
      expect(thinking.length, greaterThan(4000));
      // 头部必须还在：旧实现从 4000 字起把开头裁掉，用户看到「前面的字被吞」。
      expect(thinking.startsWith(chunk), isTrue);
      expect(thinking, List.filled(30, chunk).join());

      // 整包 assistant 到达后仍以全量覆盖，内容不丢。
      model.apply({
        'type': 'assistant',
        'message': {
          'content': [
            {'type': 'thinking', 'thinking': thinking},
          ],
        },
      });
      expect(lastOf(model, ChatEntryKind.thinking).text, thinking);
    },
  );

  test('long streaming assistant text keeps its beginning too', () {
    final model = ChatModel()..addUser('q');
    final chunk = List.filled(250, '回答内容').join(); // 每次 1000 字
    for (var i = 0; i < 6; i++) {
      model.apply({
        'type': 'stream_event',
        'event': {
          'type': 'content_block_delta',
          'delta': {'type': 'text_delta', 'text': chunk},
        },
      });
    }
    final text = lastOf(model, ChatEntryKind.assistant).text;
    expect(text.length, greaterThan(4000));
    expect(text.startsWith(chunk), isTrue);
  });

  test('tool_use and tool_result are paired by tool_use_id', () {
    final model = ChatModel()..addUser('列目录');
    model.apply({
      'type': 'assistant',
      'message': {
        'content': [
          {
            'type': 'tool_use',
            'id': 't1',
            'name': 'Cmd',
            'input': {'command': 'dir'},
          },
        ],
      },
    });
    final tool = lastOf(model, ChatEntryKind.toolUse);
    expect(tool.toolName, 'Cmd');
    expect(tool.toolUseId, 't1');
    expect(tool.detail, contains('dir'));

    model.apply({
      'type': 'user',
      'message': {
        'content': [
          {'type': 'tool_result', 'tool_use_id': 't1', 'content': 'a.txt'},
        ],
      },
    });
    final result = lastOf(model, ChatEntryKind.toolResult);
    expect(result.toolUseId, 't1');
    expect(result.text, 'a.txt');
  });

  test('approval requests surface as a card and cancel marks them handled', () {
    final model = ChatModel()..addUser('关机');
    model.apply({
      'type': 'control_request',
      'request_id': 'req1',
      'request': {
        'subtype': 'can_use_tool',
        'tool_name': 'Cmd',
        'tool_use_id': 't1',
        'input': {'command': 'shutdown /s'},
      },
    });
    final approval = lastOf(model, ChatEntryKind.approval);
    expect(approval.requestId, 'req1');
    expect(approval.toolUseId, 't1');
    expect(approval.toolName, 'Cmd');
    expect(approval.handled, isFalse);
    expect(model.busy, isTrue);

    model.apply({'type': 'control_cancel_request', 'request_id': 'req1'});
    expect(approval.handled, isTrue);
  });

  test('agent restart invalidates cards that can no longer be answered', () {
    final model = ChatModel()..addUser('跑两个命令');
    for (final item in const [
      ('req1', 't1', 'dir'),
      ('req2', 't2', 'whoami'),
    ]) {
      model.apply({
        'type': 'control_request',
        'request_id': item.$1,
        'request': {
          'subtype': 'can_use_tool',
          'tool_name': 'Cmd',
          'tool_use_id': item.$2,
          'input': {'command': item.$3},
        },
      });
    }
    final cards = model.entries
        .where((entry) => entry.kind == ChatEntryKind.approval)
        .toList();
    expect(cards, hasLength(2));

    // 新拉起的 agent 不认旧 request_id：两张卡一起作废，按钮不再留着骗人。
    expect(model.invalidateApprovals('agent 已重启'), 2);
    expect(cards.every((card) => card.handled), isTrue);
    expect(model.invalidateApprovals('再来一次'), 0);
  });

  test('result ends the turn and clears any unanswered approvals', () {
    final model = ChatModel()..addUser('q');
    model.apply({
      'type': 'control_request',
      'request_id': 'req9',
      'request': {
        'subtype': 'can_use_tool',
        'tool_name': 'Cmd',
        'tool_use_id': 't9',
        'input': {'command': 'dir'},
      },
    });
    final card = lastOf(model, ChatEntryKind.approval);
    expect(card.handled, isFalse);

    model.apply({'type': 'result', 'subtype': 'success', 'is_error': false});
    expect(card.handled, isTrue);
    expect(model.busy, isFalse);
  });

  test('a paused round ends with a notice instead of looking like success', () {
    // §待办 3-28.1：暂停是「在轮首安全点停下」，不是失败、也不该和正常收尾长得一样。
    final model = ChatModel()..addUser('跑一个长任务');
    model.apply({
      'type': 'result',
      'subtype': 'paused',
      'usage': {'in': 10, 'out': 2, 'read': 0, 'create': 0},
    });
    expect(model.busy, isFalse);
    expect(
      model.entries.any((entry) => entry.kind == ChatEntryKind.error),
      isFalse,
    );
    final notice = lastOf(model, ChatEntryKind.notice);
    expect(notice.text, contains('已暂停'));
    expect(notice.text, contains('继续'));
  });

  test('result with is_error becomes an error entry and ends the turn', () {
    final model = ChatModel()..addUser('q');
    model.apply({
      'type': 'result',
      'subtype': 'error_during_execution',
      'is_error': true,
      'result': '接口 401',
    });
    expect(lastOf(model, ChatEntryKind.error).text, '接口 401');
    expect(model.busy, isFalse);
  });

  test('non-streaming result text is shown as one assistant message', () {
    final model = ChatModel()..addUser('q');
    model.apply({
      'type': 'result',
      'subtype': 'success',
      'is_error': false,
      'result': '最终回答',
    });
    expect(lastOf(model, ChatEntryKind.assistant).text, '最终回答');
  });

  test('usage and init model are recorded', () {
    final model = ChatModel()..addUser('q');
    model.apply({
      'type': 'system',
      'subtype': 'init',
      'model': 'deepseek-chat',
    });
    model.apply({
      'type': 'usage_delta',
      'usage': {'in': 12, 'read': 3, 'create': 0, 'out': 4},
    });
    expect(model.modelName, 'deepseek-chat');
    expect(model.inputTokens, 12);
    expect(model.outputTokens, 4);
  });

  test('history snapshot carries id, title and both sides of the chat', () {
    final model = ChatModel()..addUser('  帮我 整理\n这个目录  ');
    // 只有用户消息时也能落盘（v1：会话失败 / 中途关掉也要留在历史里）。
    expect(model.historyJson(), isNotNull);
    model.apply({
      'type': 'result',
      'subtype': 'success',
      'is_error': false,
      'result': '好的',
    });
    final snapshot = model.historyJson()!;
    expect(snapshot['id'], model.sessionId);
    // 标题取首条用户消息、压掉换行与多余空格。
    expect(snapshot['title'], '帮我 整理 这个目录');
    final messages = (snapshot['messages'] as List).cast<Map>();
    expect(messages.map((item) => item['role']).toList(), [
      'user',
      'assistant',
    ]);
    expect(messages.last['content'], '好的');
  });

  test('task drawer titles summarize content instead of numbering', () {
    expect(summarizeTaskTitle('  ## 帮我修设置窗口的拖动\n第二行细节'), '帮我修设置窗口的拖动');
    expect(summarizeTaskTitle('\n\n', fallback: '过程记录'), '过程记录');
    const long =
        '这是一条很长很长的任务说明，用来验证标题只保留主要内容而不会把整段提示词塞进抽屉标题。'
        '后面还有很多细节，不应该全部出现在标题里，否则抽屉头会被撑爆。';
    final title = summarizeTaskTitle(long);
    expect(title.runes.length, lessThanOrEqualTo(49));
    expect(title.endsWith('…'), isTrue);
    expect(title.contains('任务 1'), isFalse);
  });

  test('task drawer title prefers the matching TodoWrite item', () {
    final model = ChatModel()..addUser('把设置窗口的拖动修一下');
    model.apply({
      'type': 'assistant',
      'message': {
        'content': [
          {
            'type': 'tool_use',
            'id': 't1',
            'name': 'TodoWrite',
            'input': {
              'todos': [
                {'content': '定位标题栏拖拽层', 'status': 'completed'},
                {'content': '给设置标题栏接上拖动', 'status': 'in_progress'},
              ],
            },
          },
        ],
      },
    });

    expect(model.todoTitleForTurn(0), '给设置标题栏接上拖动');
    expect(model.todoTitleForTurn(1), isNull);
  });

  test('restore puts back user questions missing from old process steps', () {
    final model = ChatModel()
      ..restore(
        id: 's-old-steps',
        messages: const [
          ChatMessage('user', '最早的问题：设置窗口拖不动'),
          ChatMessage('assistant', '先看代码'),
          ChatMessage('user', '继续修'),
          ChatMessage('assistant', '好了'),
        ],
        steps: const [
          // 旧版 steps 体积上限把最早那条用户提问截掉了。
          {'kind': 'assistant', 'text': '先看代码'},
          {'kind': 'user', 'text': '继续修'},
          {'kind': 'assistant', 'text': '好了'},
        ],
      );

    expect(
      model.entries
          .where((entry) => entry.kind == ChatEntryKind.user)
          .map((entry) => entry.text),
      ['最早的问题：设置窗口拖不动', '继续修'],
    );
    // 过程块仍在：不是退回「只有正文」的降级形态。
    expect(model.entries.length, 4);
  });

  test('restore rebuilds entries and can drop the id on delete', () {
    final model = ChatModel();
    model.restore(
      id: 'sess-1',
      createdAt: DateTime.fromMillisecondsSinceEpoch(1700000000000),
      messages: const [
        ChatMessage('user', '旧问题'),
        ChatMessage('assistant', '旧回答'),
      ],
    );
    expect(model.sessionId, 'sess-1');
    expect(model.entries.map((entry) => entry.text).toList(), ['旧问题', '旧回答']);
    expect(model.busy, isFalse);

    model.forgetSessionId();
    expect(model.sessionId, isNull);
    // 消息还在，但快照暂时没有 id（下次 addUser 会重新分配）。
    expect(model.messages.length, 2);
    expect(model.historyJson(), isNull);
  });

  test('elicitation request becomes a form with parsed schema fields', () {
    final model = ChatModel()..addUser('导出');
    model.apply({
      'type': 'control_request',
      'request_id': 'r1',
      'request': {
        'subtype': 'elicitation',
        'message': '请补充导出参数',
        'requestedSchema': {
          'type': 'object',
          'properties': {
            'format': {
              'type': 'string',
              'title': '格式',
              'enum': ['csv', 'xlsx'],
              'default': 'xlsx',
            },
            'rows': {'type': 'integer', 'title': '行数', 'default': 10},
            'zip': {'type': 'boolean', 'title': '压缩'},
          },
          'required': ['format'],
        },
      },
    });
    final card = lastOf(model, ChatEntryKind.approval);
    expect(card.subtype, 'elicitation');
    expect(card.requestId, 'r1');
    expect(card.elicitMessage, '请补充导出参数');
    expect(card.elicitFields.map((field) => field.key).toList(), [
      'format',
      'rows',
      'zip',
    ]);
    final format = card.elicitFields.first;
    expect(format.options, ['csv', 'xlsx']);
    expect(format.defaultValue, 'xlsx');
    expect(format.required, isTrue);
    // 数字默认值也转成字符串供输入框回填。
    expect(card.elicitFields[1].defaultValue, '10');
    expect(model.busy, isTrue);
  });

  test('mcp_trust and mcp_sampling keep their own payloads', () {
    final model = ChatModel()..addUser('q');
    model.apply({
      'type': 'control_request',
      'request_id': 'r2',
      'request': {
        'subtype': 'mcp_trust',
        'project': 'D:/work/demo',
        'servers': [
          {'name': 'fs', 'transport': 'stdio', 'target': 'npx fs-server'},
        ],
      },
    });
    final trust = lastOf(model, ChatEntryKind.approval);
    expect(trust.subtype, 'mcp_trust');
    expect(trust.trustProject, 'D:/work/demo');
    expect(trust.trustServers.single.name, 'fs');
    expect(trust.trustServers.single.target, 'npx fs-server');

    model.apply({
      'type': 'control_request',
      'request_id': 'r3',
      'request': {
        'subtype': 'mcp_sampling',
        'server': 'fs',
        'messageCount': 3,
        'maxTokens': 1024,
        'viaHttp': true,
      },
    });
    final sampling = lastOf(model, ChatEntryKind.approval);
    expect(sampling.subtype, 'mcp_sampling');
    expect(sampling.samplingServer, 'fs');
    expect(sampling.samplingMessages, 3);
    expect(sampling.samplingMaxTokens, 1024);
    expect(sampling.samplingViaHttp, isTrue);
  });

  test('unknown control requests stay a notice instead of a fake card', () {
    final model = ChatModel()..addUser('q');
    model.apply({
      'type': 'control_request',
      'request_id': 'r4',
      'request': {'subtype': 'something_new'},
    });
    expect(
      model.entries.where((entry) => entry.kind == ChatEntryKind.approval),
      isEmpty,
    );
    expect(lastOf(model, ChatEntryKind.notice).text, contains('something_new'));
  });

  test('manual compact reuses its placeholder note when the agent replies', () {
    final model = ChatModel()..addUser('q');
    model.markCompactRequested();
    final note = lastOf(model, ChatEntryKind.notice);
    expect(note.text, contains('已请求压缩'));

    model.apply({
      'type': 'system',
      'subtype': 'context_compacted',
      'elided': 12,
      'dropped': 5,
    });
    expect(note.text, '上下文已压缩：省略 12 个工具结果，丢弃 5 条早期消息');
    // 原地改文字：占位与结果合并成同一条，不额外多一条。
    expect(
      model.entries.where((entry) => entry.kind == ChatEntryKind.notice).length,
      1,
    );
  });

  test('steer_inserted lands as a receipt notice for the mid-turn insert', () {
    // §待办 3-28.1「立即插入」：用户按发送时只是发出去了，真正生效在 agent 的
    // **轮首安全点**；这条回执是界面上唯一能证明「消息没丢」的地方。
    final model = ChatModel()..addUser('插入');
    model.apply({'type': 'system', 'subtype': 'steer_inserted', 'chars': 17});
    expect(lastOf(model, ChatEntryKind.notice).text, contains('轮首生效'));
    expect(lastOf(model, ChatEntryKind.notice).text, contains('17 字'));
    // 字数缺失时也要给一句完整的话（不出现「，字」这种残句）。
    model.apply({'type': 'system', 'subtype': 'steer_inserted'});
    expect(lastOf(model, ChatEntryKind.notice).text, '已插入本回合（轮首生效）');
  });

  test(
    'fork copies up to the reply and leaves the original session intact',
    () {
      // §待办 3-28.6：分叉是「复制」，不是「回退」—— 原会话那 4 条必须一条不少。
      final model = ChatModel()..addUser('问题一');
      model.apply({
        'type': 'assistant',
        'message': {
          'content': [
            {'type': 'text', 'text': '回答一'},
          ],
        },
      });
      model.addUser('问题二');
      model.apply({
        'type': 'assistant',
        'message': {
          'content': [
            {'type': 'text', 'text': '回答二'},
          ],
        },
      });
      final firstReply = model.entries.firstWhere(
        (entry) => entry.kind == ChatEntryKind.assistant,
      );

      final snapshot = model.forkSnapshotAt(
        firstReply,
        existingTitles: {'问题一（分支）'},
      )!;
      expect(snapshot['title'], '问题一（分支 2）', reason: '重名要加编号，不能两条同名');
      final kept = snapshot['messages']! as List;
      expect(kept.length, 2, reason: '只到这条回答为止');
      expect((kept.last as Map)['content'], '回答一');
      expect(model.messages.length, 4, reason: '原会话没有被截断');

      // 不是助手条目 / 不在正文里 ⇒ 不造分支。
      expect(model.forkSnapshotAt(model.entries.first), isNull);
    },
  );

  test('activity view is derived from the same entries as the chat', () {
    // §待办 3-28.2：活动面板不另存状态 —— 有 tool_use 没 tool_result 就是「在跑」，
    // 审批卡没处理就是「待审批」。同一份 entries，两边不可能说两套话。
    final model = ChatModel()..addUser('看看项目');
    model.apply({
      'type': 'assistant',
      'message': {
        'content': [
          {
            'type': 'tool_use',
            'id': 't1',
            'name': 'Read',
            'input': {'file_path': 'a.dart'},
          },
          {
            'type': 'tool_use',
            'id': 't2',
            'name': 'Agent',
            'input': {'description': '找配置'},
          },
        ],
      },
    });
    expect(model.runningTools.map((entry) => entry.toolUseId), ['t1', 't2']);
    expect(model.agentCalls.single.toolUseId, 't2');

    model.apply({
      'type': 'user',
      'message': {
        'content': [
          {'type': 'tool_result', 'tool_use_id': 't1', 'content': 'ok'},
        ],
      },
    });
    expect(model.runningTools.map((entry) => entry.toolUseId), ['t2']);
    expect(model.finishedToolIds, contains('t1'));

    model.apply({
      'type': 'control_request',
      'request_id': 'r1',
      'request': {
        'subtype': 'can_use_tool',
        'tool_name': 'Write',
        'tool_use_id': 't9',
      },
    });
    expect(model.pendingApprovals.single.toolName, 'Write');
    model.invalidateApprovals('本轮已结束');
    expect(model.pendingApprovals, isEmpty);
  });

  test('subagent usage is attributed without double counting the totals', () {
    // `result.usage.subagent` 是「其中」的一部分：只用于活动面板归因，
    // 再加进总量就是把同一笔钱算两遍。
    final model = ChatModel()..addUser('派子代理');
    model.apply({
      'type': 'result',
      'usage': {
        'input_tokens': 100,
        'output_tokens': 20,
        'cache_read_input_tokens': 30,
        'cache_creation_input_tokens': 5,
        'subagent': {
          'input_tokens': 40,
          'output_tokens': 10,
          'cache_read_input_tokens': 0,
          'cache_creation_input_tokens': 0,
          'requests': 2,
        },
      },
    });
    expect(model.subagentRequests, 2);
    expect(model.subagentTokens, 50);
    // 总量只按外层四个数结账（`inputTokens` / `outputTokens` 来自逐请求的
    // `usage_delta`，`result` 不重复记）：命中 30、未命中 100+5、输出 20。
    expect(model.hitTokens, 30);
    expect(model.missTokens, 105);
    expect(model.totalOutputTokens, 20);
  });

  test('steer_dropped retracts the bubble and hands the text back', () {
    // 回合已被预算 / 轮次收口，插入赶不上安全点 ⇒ agent 丢弃并广播。
    // 界面必须跟着撤回：那条消息从没进过 history，留着就是「发出去永远没回音」。
    final model = ChatModel()..addUser('第一问');
    model.addUser('插入的话', steerInsert: true);
    expect(
      model.entries.where((entry) => entry.kind == ChatEntryKind.user).length,
      2,
    );

    model.apply({'type': 'system', 'subtype': 'steer_dropped', 'chars': 4});

    expect(
      model.entries.where((entry) => entry.kind == ChatEntryKind.user).length,
      1,
    );
    expect(model.messages.last.content, '第一问');
    expect(model.takeDroppedSteer(), '插入的话');
    // 只还一次：取走后再取是 null，不会把同一句话重复填回输入框。
    expect(model.takeDroppedSteer(), isNull);
    expect(lastOf(model, ChatEntryKind.notice).text, contains('已撤回并放回输入框'));
  });

  test('steer_inserted clears the queue so a late drop cannot retract it', () {
    // 队列是 FIFO：一条「已注入」就出队，后面的丢弃事件只影响还在等的那条。
    final model = ChatModel()..addUser('插入的话', steerInsert: true);
    model.apply({'type': 'system', 'subtype': 'steer_inserted', 'chars': 4});
    model.apply({'type': 'system', 'subtype': 'steer_dropped', 'chars': 4});
    expect(
      model.entries.where((entry) => entry.kind == ChatEntryKind.user).length,
      1,
    );
    expect(model.takeDroppedSteer(), isNull);
    expect(lastOf(model, ChatEntryKind.notice).text, contains('没赶上本轮'));
  });

  test('init carries the context budget and thresholds from the agent', () {
    // §待办 3-28.8：预算与阈值只有 agent 一处定义，前端只读不猜。
    final model = ChatModel()..addUser('q');
    model.apply({
      'type': 'system',
      'subtype': 'init',
      'model': 'test-model',
      'max_context_tokens': 64000,
      'elide_ratio': 0.85,
      'drop_ratio': 0.95,
    });
    expect(model.contextLimit, 64000);
    expect(model.elideRatio, 0.85);
    expect(model.dropRatio, 0.95);
    expect(model.contextWaterLevel, isNull, reason: '还没发过请求，没有实测体积');
    // 老 agent（不上报这三项）⇒ 预算为 0，界面据此不画水位条（不估一个假预算）。
    final old = ChatModel()..addUser('q');
    old.apply({'type': 'system', 'subtype': 'init', 'model': 'x'});
    expect(old.contextLimit, 0);
    expect(old.contextWaterLevel, isNull);
  });

  test('system and context stats come from init and persist into history', () {
    // §待办 3-28.12 ①：SYSTEM / CONTEXT 只有字符数与哈希，没有正文。
    final shaA = 'a'.padRight(64, 'a');
    final shaB = 'b'.padRight(64, 'b');
    final model = ChatModel()..addUser('q');
    model.apply({
      'type': 'system',
      'subtype': 'init',
      'model': 'm',
      'system_prompt': {'chars': 1234, 'sha256': shaA},
      'context_block': {'present': true, 'chars': 88, 'sha256': shaB},
    });
    expect(model.systemChars, 1234);
    expect(model.systemHashShort, shaA.substring(0, 12));
    expect(model.contextPresent, isTrue);
    expect(model.contextChars, 88);
    expect(model.contextHashShort, shaB.substring(0, 12));

    // 落盘会话要带着这两栏，历史里打开的轨迹才有东西可显示。
    final json = model.historyJson();
    expect(json?['system'], {'chars': 1234, 'sha256': shaA});
    expect(json?['context'], {'present': true, 'chars': 88, 'sha256': shaB});
    final restored = ChatHistorySession.fromJson(json);
    expect(restored?.systemChars, 1234);
    expect(restored?.systemSha, shaA);
    expect(restored?.contextPresent, isTrue);
    expect(restored?.contextChars, 88);

    // 老 agent 不报这两项 ⇒ 零值，不凭空造一个哈希。
    final old = ChatModel()..addUser('q');
    old.apply({'type': 'system', 'subtype': 'init', 'model': 'x'});
    expect(old.systemSha, isEmpty);
    expect(old.contextPresent, isFalse);
    expect(old.historyJson()?.containsKey('system'), isFalse);
  });

  test('prompt timeline keeps one snapshot per init and caps at fifty', () {
    final model = ChatModel()..addUser('q');
    for (var i = 1; i <= 51; i++) {
      model.apply({
        'type': 'system',
        'subtype': 'init',
        'system_prompt': {
          'chars': 1000 + i,
          'sha256':
              '${i.toRadixString(16).padLeft(2, '0')}${'a'.padRight(62, 'a')}',
        },
        'context_block': {
          'present': i.isEven,
          'chars': i.isEven ? 10 + i : 0,
          'sha256': i.isEven ? 'b'.padRight(64, 'b') : '',
        },
      });
    }
    expect(model.promptTimeline.length, ChatModel.maxPromptSnapshots);
    expect(model.promptTimeline.first.turn, 2, reason: '丢掉最旧的一条，编号不重排');
    expect(model.promptTimeline.last.turn, 51);
    expect(model.systemChars, 1051, reason: '两栏仍是最新一次');
    expect(model.contextPresent, isFalse, reason: '第 51 轮是奇数 ⇒ 无上下文注入');

    final json = model.historyJson()!;
    final timeline = json['promptTimeline'] as List;
    expect(timeline.length, ChatModel.maxPromptSnapshots);
    expect((timeline.last as Map)['turn'], 51);
    expect(json['system'], {'chars': 1051, 'sha256': model.systemSha});

    // 历史里读回来：逐轮数组原样保留。
    final restored = ChatHistorySession.fromJson(json)!;
    expect(restored.promptTimeline.length, ChatModel.maxPromptSnapshots);
    expect(restored.promptTimeline.last.turn, 51);

    // 切回历史会话：时间线跟着 restore，最新两栏由最后一条派生。
    final replay = ChatModel()
      ..restore(
        id: 's',
        messages: const [ChatMessage('user', 'q')],
        promptTimeline: restored.promptTimeline,
      );
    expect(replay.promptTimeline.length, ChatModel.maxPromptSnapshots);
    expect(replay.systemChars, restored.systemChars);
    expect(replay.contextPresent, restored.contextPresent);
  });

  test(
    'old sessions synthesize one snapshot instead of pretending per-turn data',
    () {
      final old = ChatHistorySession.fromJson({
        'id': 'old',
        'messages': [
          {'role': 'user', 'content': 'q'},
        ],
        'system': {'chars': 1234, 'sha256': 'a'.padRight(64, 'a')},
        'context': {
          'present': true,
          'chars': 88,
          'sha256': 'b'.padRight(64, 'b'),
        },
      })!;
      expect(old.promptTimeline.length, 1);
      expect(old.promptTimeline.single.turn, 1);
      expect(old.promptTimeline.single.systemSha, 'a'.padRight(64, 'a'));
      expect(old.promptTimeline.single.contextPresent, isTrue);

      // 老 agent 连这两个键都不报 ⇒ 时间线为空（不凭空造轮次）。
      final none = ChatHistorySession.fromJson({
        'id': 'none',
        'messages': [
          {'role': 'user', 'content': 'q'},
        ],
      })!;
      expect(none.promptTimeline, isEmpty);
    },
  );

  test('skills_listing reports the budget overflow once', () {
    // §待办 3-28.11：超预算的技能不进系统提示词 ⇒ 必须提示一次（不能每轮都刷）。
    final model = ChatModel()..addUser('q');
    model.apply({
      'type': 'system',
      'subtype': 'init',
      'skills_listing': {
        'total': 60,
        'listed': 31,
        'omitted': 29,
        'budget_chars': 8000,
      },
    });
    expect(model.skillTotal, 60);
    expect(model.skillListed, 31);
    expect(model.skillOmitted, 29);
    expect(
      model.entries.where((entry) => entry.kind == ChatEntryKind.notice).length,
      1,
    );

    // 下一轮 init 带着同样的数字：不再重复提示。
    model.apply({
      'type': 'system',
      'subtype': 'init',
      'skills_listing': {
        'total': 60,
        'listed': 31,
        'omitted': 29,
        'budget_chars': 8000,
      },
    });
    expect(
      model.entries.where((entry) => entry.kind == ChatEntryKind.notice).length,
      1,
    );

    // 没超预算（或老 agent 不报）⇒ 零噪音。
    final quiet = ChatModel()..addUser('q');
    quiet.apply({
      'type': 'system',
      'subtype': 'init',
      'skills_listing': {
        'total': 3,
        'listed': 3,
        'omitted': 0,
        'budget_chars': 8000,
      },
    });
    expect(quiet.skillOmitted, 0);
    expect(quiet.entries.where((e) => e.kind == ChatEntryKind.notice), isEmpty);
  });

  test(
    'usage_delta measures the context size as in+read+create of one request',
    () {
      final model = ChatModel()..addUser('q');
      model.apply({
        'type': 'system',
        'subtype': 'init',
        'max_context_tokens': 100000,
      });
      model.apply({
        'type': 'usage_delta',
        'usage': {'in': 1000, 'read': 20000, 'create': 3000, 'out': 500},
      });
      // 一次请求的上下文 = 1000 + 20000 + 3000 = 24000；输出不计入。
      expect(model.lastContextTokens, 24000);
      expect(model.contextWaterLevel, closeTo(0.24, 1e-9));
      // 第二次请求：取**最新一次**（上下文是瞬时的，不是累计和）。
      model.apply({
        'type': 'usage_delta',
        'usage': {'in': 500, 'read': 90000, 'create': 0, 'out': 100},
      });
      expect(model.lastContextTokens, 90500);
      expect(model.contextWaterLevel, closeTo(0.905, 1e-9));
    },
  );

  test('failure hints cover the common classes with what / why / next', () {
    FailureHint? hint(String text) =>
        failureHintFor(toolName: 'Cmd', text: text);

    final denied = hint('⚠ User denied this action in Lunac')!;
    expect(denied.step, contains('Cmd'));
    expect(denied.step, contains('审批被拒'));
    expect(denied.next, contains('允许'));

    expect(hint('⚠ 工作区锁被占用')!.cause, contains('另一个'));
    expect(hint('⚠ request timed out after 120s')!.step, contains('时限'));
    expect(hint('⚠ HTTP 429 Too Many Requests')!.cause, contains('速率'));
    expect(hint('⚠ context_length_exceeded')!.next, contains('压缩'));
    expect(hint('⚠ 401 Unauthorized')!.cause, contains('密钥'));
    expect(
      hint('⚠ connect ECONNREFUSED 127.0.0.1:8080')!.step,
      contains('连不上'),
    );
    expect(hint('⚠ Access is denied.')!.cause, contains('权限'));
    expect(hint('⚠ "foo" 不是内部或外部命令，也不是可运行的程序')!.step, contains('命令不存在'));
    expect(hint('⚠ No such file or directory: a.txt')!.step, contains('找不到'));
    // 三段都必须有内容：空串会让界面出现「原因：」这种半句话。
    for (final text in [
      '⚠ User denied this action in Lunac',
      '⚠ boom',
      '⚠ HTTP 429 Too Many Requests',
    ]) {
      final item = hint(text)!;
      expect(item.step.trim(), isNotEmpty);
      expect(item.cause.trim(), isNotEmpty);
      expect(item.next.trim(), isNotEmpty);
    }
  });

  test('failure hints fall back for tools but never invent a class', () {
    // 工具卡：认不出类别也给通用三段（卡上必须能照着做点什么）。
    final generic = failureHintFor(toolName: 'Read', text: '⚠ boom')!;
    expect(generic.step, contains('Read'));
    expect(generic.cause, isNotEmpty);
    // 出错旁注（genericFallback=false）：认不出就返回 null，调用方原样显示那句人话。
    expect(
      failureHintFor(
        toolName: '',
        text: '发送失败：agent 未启动',
        genericFallback: false,
      ),
      isNull,
    );
    // 空文本不给三段。
    expect(failureHintFor(toolName: 'Read', text: '   '), isNull);
    // 误判防线：普通数字里的 4013 不算 401。
    expect(
      failureHintFor(
        toolName: 'Cmd',
        text: '⚠ build finished with 4013 warnings',
        genericFallback: false,
      ),
      isNull,
    );
  });

  test('context water levels switch at watch / elide / drop', () {
    expect(contextLevelOf(null), ContextLevel.none);
    expect(contextLevelOf(0.0), ContextLevel.calm);
    expect(contextLevelOf(0.599), ContextLevel.calm);
    expect(contextLevelOf(0.60), ContextLevel.watch);
    expect(contextLevelOf(0.849), ContextLevel.watch);
    expect(contextLevelOf(0.85), ContextLevel.elide);
    expect(contextLevelOf(0.949), ContextLevel.elide);
    expect(contextLevelOf(0.95), ContextLevel.drop);
    expect(contextLevelOf(1.0), ContextLevel.drop);
    // 阈值相等时取更危险的一档（宁可提前提示，也不显示「平静」）。
    expect(
      contextLevelOf(0.90, elideRatio: 0.90, dropRatio: 0.90),
      ContextLevel.drop,
    );
  });

  test('rewind drops the last user turn and everything after it', () {
    final model = ChatModel()..addUser('第一问');
    model.apply({
      'type': 'result',
      'subtype': 'success',
      'is_error': false,
      'result': '第一答',
    });
    model.addUser('第二问');
    model.apply({
      'type': 'result',
      'subtype': 'success',
      'is_error': false,
      'result': '第二答',
    });

    expect(model.rewindLastUser(), '第二问');
    expect(model.messages.map((message) => message.role).toList(), [
      'user',
      'assistant',
    ]);
    expect(
      model.entries.where((entry) => entry.kind == ChatEntryKind.user).length,
      1,
    );
    expect(model.entries.any((entry) => entry.text == '第二答'), isFalse);
  });

  test('rewind is refused while a turn is still running', () {
    final model = ChatModel()..addUser('q');
    expect(model.rewindLastUser(), isNull);
  });

  test('rewindTo rolls back to any earlier user message by object', () {
    final model = ChatModel()..addUser('第一问');
    model.apply({
      'type': 'result',
      'subtype': 'success',
      'is_error': false,
      'result': '第一答',
    });
    model.addUser('第二问');
    model.apply({
      'type': 'result',
      'subtype': 'success',
      'is_error': false,
      'result': '第二答',
    });

    final first = model.entries.firstWhere(
      (entry) => entry.kind == ChatEntryKind.user && entry.text == '第一问',
    );
    expect(model.rewindTo(first), '第一问');
    expect(model.messages, isEmpty);
    expect(
      model.entries.where((entry) => entry.kind == ChatEntryKind.user),
      isEmpty,
    );
    expect(model.entries.any((entry) => entry.text == '第二答'), isFalse);

    // 非用户条目不算回退目标。
    expect(
      model.rewindTo(ChatEntry(kind: ChatEntryKind.notice, text: 'x')),
      isNull,
    );

    // 回合进行中拒绝（面板先判 busy 再提示）。
    model.addUser('第三问');
    expect(model.rewindTo(model.entries.last), isNull);
  });

  test('background commands are tracked by started/done events', () {
    final model = ChatModel();
    model.apply({
      'type': 'system',
      'subtype': 'background_started',
      'id': 'bg_1',
      'pid': 4321,
      'tool_use_id': 'toolu_9',
      'label': 'npm run dev',
    });
    expect(model.backgroundCommands, hasLength(1));
    expect(model.backgroundCommands.single.toolUseId, 'toolu_9');
    expect(model.backgroundCommands.single.pid, 4321);
    expect(model.backgroundCommands.single.label, 'npm run dev');

    model.apply({'type': 'system', 'subtype': 'background_done', 'id': 'bg_1'});
    expect(model.backgroundCommands, isEmpty);
  });

  /// §待办 3-28.7：后台子代理跨回合还在跑 → 进活动面板（`backgroundTasks`）、
  /// 收到 `task_done` 收口并给一条旁注；前台的 `task_started` / `task_done` 不进这张表
  /// （它们已经有自己的 `Agent` 工具卡，两份状态必然对不上）。
  test('background subagents are tracked across started/progress/done', () {
    final model = ChatModel()..addUser('q');
    model.apply({
      'type': 'system',
      'subtype': 'task_started',
      'task_id': 'task-1',
      'description': '前台子代理',
      'role': 'explore',
    });
    expect(model.backgroundTasks, isEmpty, reason: '前台子代理不进后台表');

    model.apply({
      'type': 'system',
      'subtype': 'task_started',
      'task_id': 'task-2',
      'description': '跑长构建',
      'role': 'coding',
      'background': true,
      'started_ms': 1000,
    });
    expect(model.backgroundTasks, hasLength(1));
    expect(model.runningBackgroundTasks, 1);
    final task = model.backgroundTasks.single;
    expect(task.title, '跑长构建');
    expect(task.role, 'coding');
    expect(task.running, isTrue);
    expect(task.statusText, '在跑');

    model.apply({
      'type': 'system',
      'subtype': 'task_progress',
      'task_id': 'task-2',
      'round': 3,
      'tool': 'PowerShell',
    });
    expect(task.round, 3);
    expect(task.statusText, '在跑 · PowerShell');

    model.apply({
      'type': 'system',
      'subtype': 'task_done',
      'task_id': 'task-2',
      'ok': true,
      'partial': false,
      'ms': 42000,
      'background': true,
      'cancelled': false,
      'spent': 9876,
    });
    expect(task.running, isFalse);
    expect(task.spent, 9876);
    expect(task.elapsedMs, 42000);
    expect(model.runningBackgroundTasks, 0);
    expect(task.statusText, '已完成');
    expect(lastOf(model, ChatEntryKind.notice).text, contains('报告将在下一轮并入'));
  });

  /// 被终止的后台子代理必须如实说「已终止」，不能显示成「已完成」。
  test('a cancelled background subagent reads as stopped', () {
    final model = ChatModel()..addUser('q');
    model.apply({
      'type': 'system',
      'subtype': 'task_started',
      'task_id': 'task-3',
      'description': '被叫停的活',
      'role': 'general',
      'background': true,
    });
    model.apply({
      'type': 'system',
      'subtype': 'task_done',
      'task_id': 'task-3',
      'ok': false,
      'partial': false,
      'ms': 5000,
      'background': true,
      'cancelled': true,
      'spent': 0,
    });
    final task = model.backgroundTasks.single;
    expect(task.status, 'cancelled');
    expect(task.statusText, '已终止');
    expect(lastOf(model, ChatEntryKind.notice).text, contains('已终止'));
  });

  /// §待办 3-28.17：工具卡在参数还没发完时就要有可读状态；
  /// 半截 JSON 里只认**已闭合**的字符串值，解析不出就只报进度，绝不编造。
  test('partialToolDetail reads only closed values and never guesses', () {
    expect(partialToolDetail(''), '准备参数…');
    expect(partialToolDetail('  '), '准备参数…');

    // 已经闭合：与收完参数后的显示一致。
    expect(
      partialToolDetail('{"command":"echo hi","timeout":5}'),
      compactJson({'command': 'echo hi', 'timeout': 5}),
    );

    // 半截：command 的值已闭合 ⇒ 显示命令本身。
    expect(
      partialToolDetail('{"command":"cargo test --lib","description":"还没写完'),
      'cargo test --lib',
    );

    // 半截且字符串没闭合 ⇒ 不能拿半句话当参数，只能报进度。
    final pending = partialToolDetail('{"command":"cargo te');
    expect(pending, contains('准备参数'));
    expect(pending, contains('已收到'));
    expect(pending.contains('cargo te'), isFalse, reason: '半截字符串不能当值显示');
  });

  /// §待办 3-28.17：`content_block_start` 先摆卡，整包到达时**复用**它，
  /// 同一个工具只留一张卡，且参数从「准备中」变成最终入参。
  test('a streaming tool card is reused instead of duplicated', () {
    final model = ChatModel()..addUser('q');
    model.apply({
      'type': 'stream_event',
      'event': {
        'type': 'content_block_start',
        'index': 0,
        'content_block': {
          'type': 'tool_use',
          'id': 'toolu_1',
          'name': 'PowerShell',
          'input': <String, dynamic>{},
        },
      },
    });
    final cards = model.entries
        .where((e) => e.kind == ChatEntryKind.toolUse)
        .toList();
    expect(cards, hasLength(1), reason: '准备阶段就先有一张卡');
    expect(cards.single.toolName, 'PowerShell');
    expect(cards.single.detail, '准备参数…');

    model.apply({
      'type': 'stream_event',
      'event': {
        'type': 'content_block_delta',
        'index': 0,
        'delta': {
          'type': 'input_json_delta',
          'partial_json': '{"command":"Get-Date"}',
        },
      },
    });
    expect(cards.single.detail, contains('Get-Date'));

    model.apply({
      'type': 'assistant',
      'message': {
        'role': 'assistant',
        'content': [
          {
            'type': 'tool_use',
            'id': 'toolu_1',
            'name': 'PowerShell',
            'input': {'command': 'Get-Date'},
          },
        ],
      },
    });
    final after = model.entries
        .where((e) => e.kind == ChatEntryKind.toolUse)
        .toList();
    expect(after, hasLength(1), reason: '整包到达后不能再加一张');
    expect(after.single.detail, contains('Get-Date'));
  });

  /// `TodoWrite` 在流式阶段不建卡（它的清单在任务抽屉里），整包到达也不留空卡。
  test('a streaming TodoWrite never leaves a tool card', () {
    final model = ChatModel()..addUser('q');
    model.apply({
      'type': 'stream_event',
      'event': {
        'type': 'content_block_start',
        'index': 0,
        'content_block': {
          'type': 'tool_use',
          'id': 'toolu_todo',
          'name': 'TodoWrite',
          'input': <String, dynamic>{},
        },
      },
    });
    expect(
      model.entries.where((e) => e.kind == ChatEntryKind.toolUse),
      isEmpty,
    );
    model.apply({
      'type': 'assistant',
      'message': {
        'role': 'assistant',
        'content': [
          {
            'type': 'tool_use',
            'id': 'toolu_todo',
            'name': 'TodoWrite',
            'input': {
              'todos': [
                {'content': '做一件事', 'status': 'pending'},
              ],
            },
          },
        ],
      },
    });
    expect(
      model.entries.where((e) => e.kind == ChatEntryKind.toolUse),
      isEmpty,
      reason: 'TodoWrite 永远不占命令卡',
    );
    expect(model.todos, hasLength(1));
  });

  test('plan mode banner is a singleton that follows the agent state', () {
    final model = ChatModel()..addUser('q');
    model.apply({'type': 'system', 'subtype': 'plan_mode', 'state': 'on'});
    final note = lastOf(model, ChatEntryKind.notice);
    expect(note.text, startsWith('计划模式'));

    model.apply({
      'type': 'system',
      'subtype': 'plan_mode',
      'state': 'off',
      'reason': '用户批准',
    });
    expect(note.text, '计划已批准，写权限恢复 · 用户批准');
    expect(
      model.entries.where((entry) => entry.kind == ChatEntryKind.notice).length,
      1,
    );
  });

  test('hook notes and push notifications land as readable notices', () {
    final model = ChatModel()..addUser('q');
    model.apply({
      'type': 'system',
      'subtype': 'hook_note',
      'hook_event': 'PreToolUse',
      'tool_name': 'Cmd',
      'items': [
        {'kind': 'block', 'text': '命令被拦截', 'command': 'rm -rf'},
      ],
    });
    final hook = lastOf(model, ChatEntryKind.notice);
    expect(hook.text, contains('hook PreToolUse · Cmd · 1 条'));
    expect(hook.text, contains('[block] 命令被拦截 — rm -rf'));

    model.apply({
      'type': 'system',
      'subtype': 'push_notification',
      'notification': {'title': '完成', 'body': '任务已结束'},
    });
    expect(lastOf(model, ChatEntryKind.notice).text, '通知：完成 — 任务已结束');
  });

  test('send-user-file becomes a file card with metadata', () {
    final model = ChatModel()..addUser('q');
    model.apply({
      'type': 'system',
      'subtype': 'user_file',
      'file': {
        'path': r'C:\out\report.md',
        'name': 'report.md',
        'kind': 'text',
        'size': 2048,
        'preview': '# 报告',
      },
    });
    final card = lastOf(model, ChatEntryKind.file);
    expect(card.filePath, r'C:\out\report.md');
    expect(card.fileName, 'report.md');
    expect(card.fileKind, 'text');
    expect(card.fileSize, 2048);
    expect(card.filePreview, '# 报告');
  });

  test('TodoWrite updates the todo list without leaving tool cards', () {
    final model = ChatModel()..addUser('做三件事');
    model.apply({
      'type': 'assistant',
      'message': {
        'content': [
          {
            'type': 'tool_use',
            'id': 't1',
            'name': 'TodoWrite',
            'input': {
              'todos': [
                {'content': '第一步', 'status': 'completed'},
                {'content': '第二步', 'status': 'in_progress'},
              ],
            },
          },
        ],
      },
    });
    expect(model.todos.map((todo) => todo.content).toList(), ['第一步', '第二步']);
    expect(model.todos.last.status, 'in_progress');
    expect(
      model.entries.where((entry) => entry.kind == ChatEntryKind.toolUse),
      isEmpty,
    );

    // 清单的结果也不进对话流（与 v1 同口径）。
    model.apply({
      'type': 'user',
      'message': {
        'content': [
          {'type': 'tool_result', 'tool_use_id': 't1', 'content': 'ok'},
        ],
      },
    });
    expect(
      model.entries.where((entry) => entry.kind == ChatEntryKind.toolResult),
      isEmpty,
    );

    // 下一轮是完整清单：整体替换，不是追加。
    model.apply({
      'type': 'assistant',
      'message': {
        'content': [
          {
            'type': 'tool_use',
            'id': 't2',
            'name': 'TodoWrite',
            'input': {
              'todos': [
                {'content': '全部完成', 'status': 'completed'},
              ],
            },
          },
        ],
      },
    });
    expect(model.todos.length, 1);
    expect(model.todos.single.status, 'completed');
  });

  test('compactJson collapses whitespace and truncates long payloads', () {
    expect(compactJson({'a': 1}), '{"a":1}');
    expect(compactJson(null), '');
    final long = compactJson({'command': 'x' * 1000});
    expect(long.length, lessThanOrEqualTo(401));
    expect(long.endsWith('…'), isTrue);
  });

  test('changedFilesOf trusts only the write tools and de-duplicates', () {
    // §待办 1-8：过程块头部的「已修改 …」只从写类工具的精确入参来。
    ChatEntry tool(String name, [String? detail]) =>
        ChatEntry(kind: ChatEntryKind.toolUse, toolName: name, detail: detail);

    final files = changedFilesOf([
      tool('Edit', r'{"file_path":"D:\\cc\\a.dart"}'),
      tool('Write', r'{"file_path":"D:\\cc\\b.md"}'),
      tool('NotebookEdit', r'{"notebook_path":"D:\\cc\\c.ipynb"}'),
      // 命令类不冒充：`cargo fmt` / 脚本 / `git` 都可能改文件，但无法精确知道。
      tool('Cmd', r'{"command":"cargo fmt"}'),
      tool('PowerShell', r'{"command":"Get-ChildItem"}'),
      // 同一个文件（只有大小写不同）只算一次。
      tool('Edit', r'{"file_path":"d:\\cc\\A.DART"}'),
      // 读类工具不算改动。
      tool('Read', r'{"file_path":"D:\\cc\\z.dart"}'),
      // 分片 / 非法 JSON 一律跳过，不猜。
      tool('Edit', r'{"file_path":"D:\\\\cc'),
      tool('Edit'),
    ]);

    expect(files, [r'D:\cc\a.dart', r'D:\cc\b.md', r'D:\cc\c.ipynb']);
    expect(changedFilesOf(const []), isEmpty);
  });

  test('restored sessions rebuild the changed-file list from their steps', () {
    // v1 从 `SessionStep.path` 重建任务抽屉的「更改文件」；v2 用同一份过程快照。
    final steps = <dynamic>[
      {'kind': 'user', 'text': '改两个文件'},
      {
        'kind': 'toolUse',
        'name': 'Edit',
        'id': 't1',
        'detail': r'{"file_path":"lib\\main.dart"}',
      },
      {
        'kind': 'toolUse',
        'name': 'Write',
        'id': 't2',
        'detail': r'{"file_path":"D:\\cc\\b.md"}',
      },
      {
        'kind': 'toolUse',
        'name': 'Cmd',
        'id': 't3',
        'detail': r'{"command":"cargo fmt"}',
      },
      {'kind': 'toolResult', 'id': 't1', 'text': 'ok'},
    ];
    final files = changedFilesFromSteps(
      steps,
      resolve: (path) => path.startsWith(r'D:\') ? path : 'D:\\work\\$path',
    );
    expect(files, [r'D:\work\lib\main.dart', r'D:\cc\b.md']);
    // 不给 resolve 时原样返回（工作区口径由调用方决定，模型不猜）。
    expect(changedFilesFromSteps(steps), [r'lib\main.dart', r'D:\cc\b.md']);
    // 非工具步骤 / 坏步骤一律跳过。
    expect(
      changedFilesFromSteps([
        {'kind': 'nope'},
        'not a map',
      ]),
      isEmpty,
    );
  });

  test('changedFilesLabel names one file and counts several', () {
    // §待办 1-8 的验收文案：单个文件写文件名，多个写「已修改 N 个文件」，没有就空。
    expect(changedFilesLabel(const []), '');
    expect(
      changedFilesLabel([r'D:\cc\lunac-master\app-flutter\lib\main.dart']),
      '已修改 main.dart',
    );
    expect(
      changedFilesLabel([
        r'D:\cc\lunac-master\app-flutter\lib\main.dart',
        r'D:\cc\lunac-master\docs\todo.md',
        r'D:\cc\lunac-master\app-flutter\rust\src\lib.rs',
      ]),
      '已修改 3 个文件',
    );
  });

  test('process blocks survive a history round trip', () {
    final model = ChatModel()..addUser('把 a.dart 改一下');
    model.apply({
      'type': 'assistant',
      'message': {
        'content': [
          {'type': 'thinking', 'thinking': '先看文件'},
          {
            'type': 'tool_use',
            'id': 't1',
            'name': 'Edit',
            'input': {r'file_path': r'D:\cc\a.dart'},
          },
          {'type': 'text', 'text': '改好了'},
        ],
      },
    });
    model.apply({
      'type': 'user',
      'message': {
        'content': [
          {'type': 'tool_result', 'tool_use_id': 't1', 'content': 'ok'},
        ],
      },
    });

    final snapshot = model.historyJson()!;
    final steps = snapshot['steps'] as List;
    expect(steps, isNotEmpty);

    final restored = ChatModel()
      ..restore(
        id: snapshot['id'] as String,
        messages: [
          for (final message in snapshot['messages'] as List)
            ChatMessage(
              (message as Map)['role'] as String,
              message['content'] as String,
            ),
        ],
        steps: steps,
      );

    // 条目顺序与类型逐个还原（思考 / 工具卡 / 正文 / 工具结果）。
    expect(
      restored.entries.map((entry) => entry.kind).toList(),
      model.entries.map((entry) => entry.kind).toList(),
    );
    expect(restored.entries.first.text, '把 a.dart 改一下');
    // 「已修改 …」的文件维度同样还原：切回历史会话仍知道改过哪些文件。
    expect(
      changedFilesOf(
        restored.entries.where((entry) => entry.kind == ChatEntryKind.toolUse),
      ),
      [r'D:\cc\a.dart'],
    );
  });

  test('old sessions without steps fall back to plain messages', () {
    final restored = ChatModel()
      ..restore(
        id: 's-old',
        messages: [
          const ChatMessage('user', '你好'),
          const ChatMessage('assistant', '你好呀'),
        ],
      );

    expect(restored.entries.map((entry) => entry.kind).toList(), [
      ChatEntryKind.user,
      ChatEntryKind.assistant,
    ]);
  });

  test('turn tool stats group into the six v1 categories', () {
    ChatEntry tool(String name) =>
        ChatEntry(kind: ChatEntryKind.toolUse, toolName: name);

    final groups = groupToolsByCategory([
      tool('Write'),
      tool('Edit'),
      tool('NotebookEdit'),
      tool('Read'),
      tool('Read'),
      tool('Grep'),
      tool('Glob'),
      tool('WebSearch'),
      tool('WebFetch'),
      tool('Cmd'),
      tool('PowerShell'),
      // 不在六类里的工具（清单 / MCP 自定义工具 / 后台任务）不计入。
      tool('TodoWrite'),
      tool('mcp__foo__bar'),
      tool('BashOutput'),
    ]);

    expect(groups.keys.toSet(), toolCategoryOrder.toSet());
    expect(groups['create'], hasLength(1));
    expect(groups['edit'], hasLength(2));
    expect(groups['read'], hasLength(2));
    expect(groups['search'], hasLength(2));
    expect(groups['web'], hasLength(2));
    expect(groups['command'], hasLength(2));
    expect(toolCategoryOf('Read'), 'read');
    expect(toolCategoryOf('BashOutput'), isNull);
  });

  test('turn durations are recorded, persisted and trimmed by rewind', () {
    final model = ChatModel()..addUser('第一轮');
    model.apply({'type': 'result', 'result': '好了'});
    expect(model.turnDurationsMs, hasLength(1));

    model.addUser('第二轮');
    model.apply({'type': 'result', 'result': '又好了'});
    expect(model.turnDurationsMs, hasLength(2));

    final snapshot = model.historyJson()!;
    expect(snapshot['turnDurations'], hasLength(2));

    // 回退到第二轮之前：耗时表跟着砍（信息条不能留着已删回合的数字）。
    final firstUser = model.entries.firstWhere(
      (entry) => entry.kind == ChatEntryKind.user && entry.text == '第二轮',
    );
    expect(model.rewindTo(firstUser), '第二轮');
    expect(model.turnDurationsMs, hasLength(1));

    // 历史往返：耗时随会话恢复。
    final durations = [
      for (final item in snapshot['turnDurations'] as List)
        (item as num).toInt(),
    ];
    final restored = ChatModel()
      ..restore(
        id: snapshot['id'] as String,
        messages: const [ChatMessage('user', '第一轮')],
        turnDurations: [durations.first],
      );
    expect(restored.turnDurationsMs, hasLength(1));
  });

  test('history metadata parses without loading messages or steps', () {
    final meta = ChatHistoryMeta.fromJson({
      'id': 's1',
      'title': '鉴权接口',
      'createdAt': 1700000000000,
      'updatedAt': 1700000100000,
      'messageCount': 7,
    });
    expect(meta, isNotNull);
    expect(meta!.id, 's1');
    expect(meta.title, '鉴权接口');
    expect(meta.messageCount, 7);
    expect(meta.createdAt!.millisecondsSinceEpoch, 1700000000000);
    expect(meta.updatedAt!.millisecondsSinceEpoch, 1700000100000);

    // 坏行 / 空 id 直接丢掉，列表不会出现点不开的幽灵行。
    expect(ChatHistoryMeta.fromJson({'title': '没有 id'}), isNull);
    expect(ChatHistoryMeta.fromJson({'id': ''}), isNull);
    expect(ChatHistoryMeta.fromJson('not a map'), isNull);
    // 老数据缺字段也要能读：标题兜底、计数归零。
    final legacy = ChatHistoryMeta.fromJson({'id': 's2'});
    expect(legacy!.title, '未命名对话');
    expect(legacy.messageCount, 0);
    expect(legacy.createdAt, isNull);
  });
}
