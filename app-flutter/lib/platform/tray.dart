// 平台薄层：系统托盘（M1）。
//
// 作用：无边框窗口没有标题栏按钮，Esc / 托盘左键都能把主窗口隐藏；托盘菜单是
// 「显示 / 退出」的固定入口（与 v1 的托盘语义一致，见 docs/todo.md §待办 1-14）。
// 图标资产 = `assets/tray_icon.png`（由 scripts\make-tray-icon.ps1 生成）。
// 所有 `tray_manager` 调用都收口在这里，UI 代码不直接依赖该插件。
import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:tray_manager/tray_manager.dart';

import 'app_window.dart';

/// 托盘 / 菜单对象必须被长期持有：nativeapi 的包装对象一旦被 GC 就会释放原生句柄，
/// 图标会随之消失。启动日志里会读一次这三个字段，作为「托盘就绪」的证据。
TrayIcon? _trayIcon;
Menu? _trayMenu;
final List<MenuItem> _menuItems = <MenuItem>[];

/// 创建托盘图标与菜单；失败时只记一行日志，不影响主窗口启动。
Future<void> initTray() async {
  try {
    final icon = TrayIcon.create();
    if (icon == null) {
      debugPrint('Lunac tray: native tray icon is not supported on this system');
      return;
    }

    icon.icon = ImageAsset.fromAsset('assets/tray_icon.png');
    icon.setTooltip('Lunac');

    final menu = Menu.create();
    if (menu != null) {
      // 右键菜单是系统模态弹窗：打开期间主窗不能隐藏（否则菜单会被一起关掉，
      // 用户看到的就是「闪一下就消失」）。
      menu.addListener((event) {
        if (event is MenuOpenedEvent) {
          setTrayMenuOpen(true);
        } else if (event is MenuClosedEvent) {
          setTrayMenuOpen(false);
        }
      });
      final showItem =
          MenuItem.createWithLabelAndType('显示 Lunac', MenuItemType.normal);
      final quitItem =
          MenuItem.createWithLabelAndType('退出 Lunac', MenuItemType.normal);
      if (showItem != null && quitItem != null) {
        showItem.addListener((event) {
          if (event is MenuItemClickedEvent) {
            unawaited(showMainWindow());
          }
        });
        quitItem.addListener((event) {
          if (event is MenuItemClickedEvent) {
            unawaited(quitApp());
          }
        });
        menu.addItem(showItem);
        menu.addSeparator();
        menu.addItem(quitItem);
        icon.setContextMenu(menu);
        icon.setContextMenuTrigger(ContextMenuTrigger.rightClicked);
        _trayMenu = menu;
        _menuItems
          ..add(showItem)
          ..add(quitItem);
      }
    }

    // 左键单击 = 显示 / 隐藏；右键交给系统弹出菜单。
    icon.addListener((event) {
      if (event is TrayIconClickedEvent) {
        unawaited(toggleMainWindow());
      }
    });
    icon.setVisible(true);
    _trayIcon = icon;
    // 退出流程（`quitApp`）先撤掉图标再关窗，避免强制退出留下「幽灵图标」。
    quitCleanup = removeTrayIcon;
    debugPrint(
      'Lunac tray ready (icon=${_trayIcon != null}, '
      'menu=${_trayMenu != null}, items=${_menuItems.length})',
    );
  } catch (e) {
    debugPrint('Lunac tray: init failed ($e)');
  }
}

/// 退出前撤掉托盘图标：先隐藏再释放原生句柄，避免留下「幽灵图标」。
void removeTrayIcon() {
  final icon = _trayIcon;
  _trayIcon = null;
  _trayMenu = null;
  _menuItems.clear();
  if (icon == null) {
    return;
  }
  try {
    icon.setVisible(false);
    icon.dispose();
  } catch (e) {
    debugPrint('Lunac tray: remove failed ($e)');
  }
}
