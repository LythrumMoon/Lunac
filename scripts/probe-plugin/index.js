// 探针插件的入口：`render.engine = "iframe"` 时宿主**不执行**它，只要求这个文件存在
// （`list_installed` 用它判「包完整」）。真正的界面在 `ui/index.html`。
export default {};
