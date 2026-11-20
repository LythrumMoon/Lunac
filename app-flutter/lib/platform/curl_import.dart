// 「命令添加」：从一段 curl 里尽力抠出供应商三要素
// （v1 `app/src/plugins/builtin/settings.ts` 的 `parseCurlConfig`，口径逐条照搬）。
//
//   · base_url —— 文本里第一个 http(s) 链接的 **origin**（scheme + host[:port]）。
//     curl 给的常是完整 endpoint，直接当 base_url 存会多出一段路径，所以被丢掉的
//     path / query 原样返回，由调用方如实提示。
//   · api_key —— `Authorization: Bearer <token>`；`$ENV` 形态不当作密钥（留空提示手填）。
//   · model   —— `--data` JSON 里的顶层 `"model"`。
//   · provider —— 主机名（去掉 `www.`），只作输入框默认值。
//
// 纯函数、不碰网络 / 文件，单测直接覆盖。

/// 一段 curl 解析出来的供应商配置（容错：能解析多少算多少）。
class CurlParsed {
  const CurlParsed({
    required this.provider,
    required this.baseUrl,
    required this.path,
    required this.apiKey,
    required this.envKey,
    required this.model,
    required this.hasKey,
  });

  /// 供应商名候选（URL 主机名，去掉 `www.`）。
  final String provider;

  /// 只取 origin（scheme + host[:port]）当作 base_url。
  final String baseUrl;

  /// curl 里那段的路径 / 查询（可能为空；未被采用，仅用于提示）。
  final String path;

  final String apiKey;

  /// Authorization 用 `$ENV` 引用时存那个引用（否则空）；值不在命令里。
  final String envKey;

  final String model;

  /// 命令里到底有没有 Authorization 头（区分「没写」与「写了但取不到值」）。
  final bool hasKey;
}

/// 解析结果：要么拿到 [parsed]，要么只有一句人话 [error]。
class CurlParseResult {
  const CurlParseResult._({this.parsed, this.error = ''});

  factory CurlParseResult.ok(CurlParsed parsed) =>
      CurlParseResult._(parsed: parsed);

  factory CurlParseResult.failed(String error) =>
      CurlParseResult._(error: error);

  final CurlParsed? parsed;
  final String error;

  bool get ok => parsed != null;
}

/// 从一段 curl 命令里解析供应商配置（找不到 URL 才算失败）。
CurlParseResult parseCurlConfig(String raw) {
  final text = raw.trim();
  final match = RegExp(
    r'https?://[^\s]+',
    caseSensitive: false,
  ).firstMatch(text);
  if (match == null) {
    return CurlParseResult.failed('没找到 URL：请粘贴完整的 curl 命令');
  }
  var rawUrl = match.group(0)!;
  rawUrl = rawUrl.replaceFirst(RegExp(r'''^["']+'''), '');
  rawUrl = rawUrl.replaceFirst(RegExp(r'''["'\\]+$'''), '');
  final uri = Uri.tryParse(rawUrl);
  if (uri == null || uri.host.isEmpty) {
    return CurlParseResult.failed('URL 解析失败：$rawUrl');
  }
  final baseUrl =
      '${uri.scheme}://${uri.host}${uri.hasPort ? ':${uri.port}' : ''}';
  final path = '${uri.path}${uri.hasQuery ? '?${uri.query}' : ''}';
  final provider = uri.host.replaceFirst(
    RegExp(r'^www\.', caseSensitive: false),
    '',
  );

  var apiKey = '';
  var envKey = '';
  var hasKey = false;
  final auth = RegExp(
    r'Authorization\s*:\s*Bearer\s+([^\s]+)',
    caseSensitive: false,
  ).firstMatch(text);
  if (auth != null) {
    hasKey = true;
    final token = auth.group(1)!.replaceFirst(RegExp(r'''["'\\]+$'''), '');
    if (token.startsWith(r'$')) {
      envKey = token;
    } else {
      apiKey = token;
    }
  }

  final modelMatch = RegExp(r'"model"\s*:\s*"([^"]+)"').firstMatch(text);
  final model = modelMatch?.group(1) ?? '';

  return CurlParseResult.ok(
    CurlParsed(
      provider: provider,
      baseUrl: baseUrl,
      path: path,
      apiKey: apiKey,
      envKey: envKey,
      model: model,
      hasKey: hasKey,
    ),
  );
}
