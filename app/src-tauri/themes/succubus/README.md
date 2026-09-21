# 魅魔 · 灰玫瑰（Lunac 主题包）

**魅魔**：夜里降临的人型少女 —— 额侧一对弯角、背后展开蝠翼，头也不回地把生活的安慰留给你。

风格是**纸上墨线**（参照 Lady Jasmin Darnell 的插画）：奶油纸底、暖黑墨线、
草稿引导线留在画面上、蕾丝与植物纹样、布料长弧，没有一处发光。
Lunac 是深色启动器，所以这里把明暗反转：**纸色成为暗底上的线稿色**，
墨色成为填充，而整幅画那唯一一个真彩 `#B06A73`（灰玫瑰裙料 / 角根细环）保留。
主色仍挂在 Lunac 原生 `--accent` `#C0A0A0` 上，**与内置默认主题同值**。

## 安装
把整个 `succubus` 目录复制到 Lunac 的 `themes\` 下：

```
<lunac.exe 所在目录>\themes\succubus\
  theme.json
  background.png          1920x1080 壁纸（墨线版；想换浅色用 svg|png/decor/wallpaper-paper）
  search_pattern.png      512x512 可平铺蕾丝花纹
  icons\                  8 个插件图标（96px PNG，CSS 按 24x24 显示）
  icons-svg\              同款 SVG 源文件（可自行改色）
```

重启 Lunac → 设置 → 风格 → 主题，选「魅魔 · 灰玫瑰」。

## 说明
- 图标是**暖彩色**位图：通过 `theme.json` 的 `assets.icons.<插件id>` 接入，
  `pluginIconSvg()` 会用 `<img>` 渲染（`.result-item-icon-img` 固定 24×24 + object-fit:contain）。
  位图出 96px，24px 下残影与点彩才留得住；1x/2x/2.5x 屏都清晰。
- 想换成自己的插件图标，把 PNG 同名替换或改 `assets.icons` 的路径；
  越界路径（含 `..`）会被后端拒绝。
- 按钮/行内图标请用 `lunac-inline/icons.ts`（`stroke:currentColor`，跟随按钮文字色）。

## 重新生成
```
cd D:\ui\_build
node build.mjs
```
