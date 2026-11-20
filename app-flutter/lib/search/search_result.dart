/// 一条搜索结果（与 Rust 侧 `lunac_search` 输出的 JSON 字段一一对应）。
///
/// 契约：`id` / `title` / `subtitle` / `kind` 四个字段必须存在；缺字段的条目在解析时丢弃，
/// 避免坏数据让整页崩掉。
class SearchResult {
  const SearchResult({
    required this.id,
    required this.title,
    required this.subtitle,
    required this.kind,
  });

  final String id;
  final String title;
  final String subtitle;

  /// `command` = 命令 / 应用内动作；`folder` = 打开本地目录；后续会加 `app` / `file`。
  final String kind;

  static SearchResult? fromJson(Map<String, dynamic> json) {
    final id = json['id'];
    final title = json['title'];
    final subtitle = json['subtitle'];
    final kind = json['kind'];
    if (id is! String || title is! String || subtitle is! String || kind is! String) {
      return null;
    }
    return SearchResult(id: id, title: title, subtitle: subtitle, kind: kind);
  }
}
