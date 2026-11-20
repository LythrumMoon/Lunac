# 规则仓库（Registry）

本仓库是「阅读」插件的公开规则仓库，发布 RSP v1 格式的聚合索引 `index.json` 与自建来源，供插件导入后使用。

## 使用方式

打开阅读插件 →「来源」→「导入规则」，URL 保持默认即可：

- jsDelivr（默认）：
  `https://cdn.jsdelivr.net/gh/LythrumMoon/lunac-rules@main/index.json`
- GitHub raw：
  `https://raw.githubusercontent.com/LythrumMoon/lunac-rules/main/index.json`

导入后：

1. 在「聚合仓库」中选择需要的条目并安装；自建来源（`manual` / `cms.*`）内嵌规则，可直接安装。
2. 也可以粘贴 `index.json` 或 `sources/` 下的来源 JSON 离线导入。
3. 在总览页输入关键词搜索已安装来源。

## 自建 CMS 采集源（`cms.*`）

`upstream/cms.json` 记录探测可用的 Apple CMS v10 采集接口，安装后由声明式执行器的
`cms` 规则完成搜索 / 总览 / 剧集 / 播放（播放页会回退到 WebView 嗅探）。
本仓库只保存接口地址，不保存任何影视内容；接口失效时在来源管理中移除即可。


## 许可证

本仓库的索引文件与自建来源采用 MIT 许可证，详见 [LICENSE](./LICENSE)。

第三方规则与内容不在本仓库内；其许可与合规由来源方及使用者自行负责。
