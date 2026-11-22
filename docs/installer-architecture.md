# 安装包前端架构（2026-11-22）

## 结论

v2 安装包主用 **Inno Setup 6.7+**，NSIS 保留为**无 ISCC 时的回退**。

| 方案 | 按钮 / 标题栏可主题化 | 单文件 | 体积 | 维护成本 | 结论 |
|---|---|---|---|---|---|
| NSIS + MUI2（旧） | 否：底部按钮是原生控件，只能涂页面背景 | 是 | 21.9 MB | 低 | 回退方案 |
| NSIS + 自绘图片按钮 | 可以，但失去原生焦点 / 键盘 / 高 DPI 行为 | 是 | 相近 | 高 | 不采用 |
| **Inno Setup 6.7+** | **是：`WizardStyle=modern dark` 统一页面、按钮、标题栏** | 是 | 23.8 MB | 中 | **主方案** |
| Flutter 自绘安装器 | 完全可控（与 Lunac 同一主题 / 动画） | 需 7z SFX 引导 | ~2 倍 | 高 | 以后要做动画再考虑 |
| WiX / MSI | 主题能力有限，适合企业分发 | MSI + bootstrapper | 中 | 高（签名 / MSIX） | 不采用 |

选择 Inno 的直接原因：NSIS 的按钮栏是系统控件，MUI 只能改页面背景色；
Inno 6.7 的内置暗色样式会同时重绘页面、导航按钮和标题栏，正好满足
「符合 Lunac 主题」。同时 Inno 的 Pascal Script 让目录页和复选项放在同一页很自然。

## 页面结构

```text
Welcome（Lunac 侧栏图）
  ↓
Select Destination Location
  [ 安装目录 ] [ Browse... ]
  [ ] Start Lunac on boot (runs in the tray at login)
  [x] Create a desktop shortcut
  ↓
Installing（进度）
  ↓
Completing
  [x] Open Lunac now
```

- 目录页的复选框由 `InitializeWizard` 创建，父窗口是 `WizardForm.DirEdit.Parent`，
  因此和目录输入框同页。
- 完成页的「Open Lunac now」来自 `[Run]` 的 `postinstall` 标志，默认勾选；
  静默安装走另一条 `--background` 启动项。
- 开机自启写 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`，值名 `Lunac`，
  命令 `"<app>\lunac.exe" --background`；取消勾选时删除旧值。
- 桌面快捷方式由 `[Icons]` + `Check: DesktopIconChecked` 控制。

## 构建

```powershell
# 主路径（需要 Inno Setup 6.7+）
powershell -ExecutionPolicy Bypass -File scripts\build-release-v2.ps1

# 也可以手动：
ISCC.exe /DMyAppVersion=1.0.1 scripts\lunac-v2-installer.iss
```

`build-release-v2.ps1` 按以下顺序找 `ISCC.exe`：

1. `<repo>\tools\InnoSetup6\ISCC.exe`（本机测试用，未入库）
2. `%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe`
3. `%ProgramFiles%\Inno Setup 6\ISCC.exe` / `%ProgramFiles(x86)%\...`
4. `PATH` 里的 `iscc`

找不到时打印 `[fallback]`，改用 `scripts\lunac-v2-installer.nsi`（`makensis`）。
构建机装一次 [Inno Setup 6.7+](https://jrsoftware.org/isdl.php) 即可。

## 主题资源

`scripts\make-installer-art.ps1` 生成 24-bit BMP（构建前自动重生成）：

| 文件 | 尺寸 | 用途 |
|---|---|---|
| `installer-welcome.bmp` | 164×314 | 欢迎 / 完成页左侧大图 |
| `installer-header.bmp` | 150×57 | NSIS 回退的页眉 |
| `installer-small.bmp` | 55×55 | Inno 右上角小图 |

颜色取 Lunac 默认主题：背景 `#16181B`、卡片 `#1B1E22`、主色 `#CFC8BB`、
次要文字 `#8B8E92`。

## 从旧 NSIS 安装迁移

- `InitializeWizard` 读旧键
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunac` 的 `InstallLocation`，
  默认目录沿用用户原来的安装位置。
- `[InstallDelete]` 删除旧 `uninstall.exe`。
- `ssPostInstall` 删除旧 NSIS 卸载键（只对真实 `Lunac` 构建，测试构建不动它）。
- 安装目录里的 `Modules\<id>\` 用户插件不会被清掉（只覆盖安装包自带文件）。
- 卸载时 `[UninstallDelete]` 删除整个 `{app}`，与旧 NSIS 行为一致。

## 验证

- 实机截图：Welcome / 目录页（目录 + 两个复选项同页）/ Completing（Open Lunac now 勾选）。
- `scripts\verify-v2.ps1` 静态守卫：
  - `WizardStyle=modern dark`
  - `AutoStartCheckBox` / `DesktopIconCheckBox` 挂在 `WizardForm.DirEdit.Parent`
  - `[Run]` 有 `postinstall`
  - 三张主题 BMP 存在
  - NSIS 回退仍有合并页 + 完成页运行选项，且不再使用 `MUI_PAGE_DIRECTORY`
- 构建：`build-release-v2.ps1` 走 Inno 分支，0 warning。
