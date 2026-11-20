cmake_minimum_required(VERSION 3.14)
project(runner LANGUAGES CXX)

# Define the application target. To change its name, change BINARY_NAME in the
# top-level CMakeLists.txt, not the value here, or `flutter run` will no longer
# work.
#
# Any new source files that you add to the application should be added here.
add_executable(${BINARY_NAME} WIN32
  "attachment_window.cpp"
  "flutter_window.cpp"
  "main.cpp"
  "plugin_window.cpp"
  "utils.cpp"
  "win32_window.cpp"
  "${FLUTTER_MANAGED_DIR}/generated_plugin_registrant.cc"
  "Runner.rc"
  "runner.exe.manifest"
)

# Apply the standard set of build settings. This can be removed for applications
# that need different build settings.
apply_standard_settings(${BINARY_NAME})

# Add preprocessor definitions for the build version.
target_compile_definitions(${BINARY_NAME} PRIVATE "FLUTTER_VERSION=\"${FLUTTER_VERSION}\"")
target_compile_definitions(${BINARY_NAME} PRIVATE "FLUTTER_VERSION_MAJOR=${FLUTTER_VERSION_MAJOR}")
target_compile_definitions(${BINARY_NAME} PRIVATE "FLUTTER_VERSION_MINOR=${FLUTTER_VERSION_MINOR}")
target_compile_definitions(${BINARY_NAME} PRIVATE "FLUTTER_VERSION_PATCH=${FLUTTER_VERSION_PATCH}")
target_compile_definitions(${BINARY_NAME} PRIVATE "FLUTTER_VERSION_BUILD=${FLUTTER_VERSION_BUILD}")

# Disable Windows macros that collide with C++ standard library functions.
target_compile_definitions(${BINARY_NAME} PRIVATE "NOMINMAX")

# Add dependency libraries and include directories. Add any application-specific
# dependencies here.
target_link_libraries(${BINARY_NAME} PRIVATE flutter flutter_wrapper_app)
target_link_libraries(${BINARY_NAME} PRIVATE "dwmapi.lib")
target_include_directories(${BINARY_NAME} PRIVATE "${CMAKE_SOURCE_DIR}")

# Run the Flutter tool portions of the build. This must not be removed.
add_dependencies(${BINARY_NAME} flutter_assemble)

# ── Rust 侧薄层（M0）──────────────────────────────────────────────
# Debug 构建取 rust/target/debug，Release 构建取 rust/target/release（2026-11-19：
# 用户明确要求出 release 安装包，打包入口 scripts\build-release-v2.ps1）。
# 这里按 `$<CONFIG>` 传给 rust/build.ps1，避免「Release 的界面 + Debug 的 DLL」。
# rust/build.ps1 负责把仓库内的 mingw 工具链补进 PATH
# （rustc 的 dlltool 查找不走 .cargo/config.toml 的 [env]）。
set(LUNAC_RUST_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../../rust")
set(LUNAC_CORE_DLL "$<IF:$<CONFIG:Debug>,${LUNAC_RUST_DIR}/target/debug/lunac_core.dll,${LUNAC_RUST_DIR}/target/release/lunac_core.dll>")
add_custom_target(lunac_core ALL
  COMMAND powershell -NoProfile -ExecutionPolicy Bypass -File "${LUNAC_RUST_DIR}/build.ps1" -Profile $<CONFIG>
  COMMENT "构建 lunac_core（Rust cdylib · $<CONFIG>）"
  VERBATIM)
add_dependencies(${BINARY_NAME} lunac_core)
add_custom_command(TARGET ${BINARY_NAME} POST_BUILD
  COMMAND ${CMAKE_COMMAND} -E copy_if_different
          "${LUNAC_CORE_DLL}" "$<TARGET_FILE_DIR:${BINARY_NAME}>/lunac_core.dll"
  COMMENT "拷贝 lunac_core.dll 到运行目录"
  VERBATIM)

# 桌宠（2026-11-12）：把 v1 已经构建好的 Live2D 运行时拷到运行目录
# `<exe>\Modules\pet\engine\`。v2 的桌宠窗（rust/src/pet.rs 的原生 WebView2 窗口）
# 通过自定义源 `https://lunac.pet/engine/...` 读这里的文件 —— 与 v1 是同一份渲染器。
# 源文件不存在时静默跳过（桌宠窗会在界面上如实报"引擎文件缺失"，不会假装渲染成功）。
set(LUNAC_PET_ENGINE_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../../../app/src-tauri/target/debug/Modules/pet/engine")
set(LUNAC_PET_ENGINE_JS "${CMAKE_CURRENT_SOURCE_DIR}/../../../app/plugin-dist/pet/engine/live2d-engine.js")
set(LUNAC_PET_ENGINE_CORE "${LUNAC_PET_ENGINE_DIR}/live2dcubismcore.min.js")
# v1 的 debug 产物可能已被清掉；release 打包仍要带上 Core（体积小），从仓库现成副本取。
if(NOT EXISTS "${LUNAC_PET_ENGINE_CORE}")
  set(LUNAC_PET_ENGINE_CORE "${CMAKE_CURRENT_SOURCE_DIR}/../../../release/deps/live2d/live2dcubismcore.min.js")
endif()
add_custom_command(TARGET ${BINARY_NAME} POST_BUILD
  COMMAND ${CMAKE_COMMAND} -E make_directory "$<TARGET_FILE_DIR:${BINARY_NAME}>/Modules/pet/engine"
  COMMENT "准备桌宠引擎目录"
  VERBATIM)
if(EXISTS "${LUNAC_PET_ENGINE_JS}")
  add_custom_command(TARGET ${BINARY_NAME} POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E copy_if_different
            "${LUNAC_PET_ENGINE_JS}"
            "$<TARGET_FILE_DIR:${BINARY_NAME}>/Modules/pet/engine/live2d-engine.js"
    COMMENT "拷贝 live2d-engine.js（与 v1 同一份）"
    VERBATIM)
endif()
if(EXISTS "${LUNAC_PET_ENGINE_CORE}")
  add_custom_command(TARGET ${BINARY_NAME} POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E copy_if_different
            "${LUNAC_PET_ENGINE_CORE}"
            "$<TARGET_FILE_DIR:${BINARY_NAME}>/Modules/pet/engine/live2dcubismcore.min.js"
    COMMENT "拷贝 live2dcubismcore.min.js（与 v1 同一份）"
    VERBATIM)
endif()

# WebView2Loader.dll（2026-11-12 实机回归）：lunac_core.dll 对它有**硬导入**
# （webview2-com-sys 动态加载器）。缺这个文件时 Dart 侧 `DynamicLibrary.open` 会
# 以 ERROR_MOD_NOT_FOUND(126) 失败，界面只会显示 "rust bridge not loaded"。
# 源文件在 rust 的构建输出里（webview2-com-sys 的 out/x64），每次构建后拷到 exe 同级。
# 若日后移除 WebView2 桌宠，这段与 rust/Cargo.toml 的 webview2-com 一起删。
add_custom_command(TARGET ${BINARY_NAME} POST_BUILD
  COMMAND powershell -NoProfile -ExecutionPolicy Bypass -Command
          "Get-ChildItem '${LUNAC_RUST_DIR}/target/debug/build' -Directory -Filter 'webview2-com-sys-*' -ErrorAction SilentlyContinue | ForEach-Object { $p = Join-Path $_.FullName 'out/x64/WebView2Loader.dll'; if (Test-Path $p) { Copy-Item -LiteralPath $p -Destination '$<TARGET_FILE_DIR:${BINARY_NAME}>/WebView2Loader.dll' -Force } }"
  COMMENT "拷贝 WebView2Loader.dll（lunac_core 的硬依赖）"
  VERBATIM)
