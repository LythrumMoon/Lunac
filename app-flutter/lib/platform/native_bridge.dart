// 平台薄层：Dart ↔ Rust 动态库的**唯一**加载入口。
//
// 边界（docs/flutter-migration.md §一）：业务逻辑不在这里；本文件只负责
// 「按平台找到 lunac_core 库 + 校验 C ABI 版本」，其余能力分别在
// `agent_status.dart`（M0 状态）与 `search_bridge.dart`（M1 搜索）里声明。
//
// 任何失败都返回**一句人话原因**而不是抛异常：桥接库可能还没构建，
// 骨架窗口仍要能起来并如实显示原因。
import 'dart:ffi';
import 'dart:io';

/// 与 `app-flutter/rust/src/lib.rs` 的 `ABI_VERSION` 必须一致。
/// M0 = 1；M1 起加了 `lunac_search` / `lunac_run_action` / `lunac_free_string`，升到 2。
const int kLunacAbiVersion = 2;

typedef _AbiVersionC = Uint32 Function();
typedef _AbiVersionDart = int Function();

/// 打开结果：要么是一份可用的库，要么是一句失败原因（二者互斥）。
class NativeOpenResult {
  const NativeOpenResult.ok(DynamicLibrary this.library) : reason = null;

  const NativeOpenResult.failed(String this.reason) : library = null;

  final DynamicLibrary? library;
  final String? reason;
}

/// 打开本机的 lunac_core 库并校验 ABI。
///
/// 每次调用都会重新 open（系统按引用计数缓存，代价可忽略）；调用方自己决定是否缓存
/// 查出的函数指针。
NativeOpenResult openLunacNative() {
  final library = _openLibrary();
  if (library == null) {
    return const NativeOpenResult.failed('rust bridge not loaded');
  }
  try {
    final abi = library
        .lookupFunction<_AbiVersionC, _AbiVersionDart>('lunac_abi_version')();
    if (abi != kLunacAbiVersion) {
      return NativeOpenResult.failed(
        'bridge ABI mismatch (dart=$kLunacAbiVersion rust=$abi)',
      );
    }
    return NativeOpenResult.ok(library);
  } catch (e) {
    return NativeOpenResult.failed('rust bridge error ($e)');
  }
}

/// 按平台加载库；Windows 找 `lunac_core.dll`，Linux 找 `liblunac_core.so`。
/// 两者都必须由构建产物放到可执行文件同目录（Windows 见 runner/CMakeLists.txt）。
DynamicLibrary? _openLibrary() {
  try {
    if (Platform.isWindows) {
      return DynamicLibrary.open('lunac_core.dll');
    }
    if (Platform.isLinux) {
      return DynamicLibrary.open('liblunac_core.so');
    }
    return null;
  } catch (_) {
    return null;
  }
}
