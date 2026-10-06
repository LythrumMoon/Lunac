// ── Live2D 引擎层的「重」那一半（L1，2026-09-30）──────────────────────
//
// 只有这里 import pixi 与 `pixi-live2d-display-lipsyncpatch`。
// **它不是 `index.js` 的一部分**：由 `scripts/build-plugins.ps1` 的 pet.`extraEntries` 单打
// 成一个自包含 ESM，落在 `<Modules>\pet\engine\live2d-engine.js`；运行时由 `pet.ts` 在
// Cubism Core 就绪之后 `import(convertFileSrc(那个绝对路径))` 拉进来。
//
// **为什么必须晚于 Core**：`pixi-live2d-display` 在**模块求值时**就
// `if (!window.Live2DCubismCore) throw`（其 dist `cubism4.es.js` 末尾）。Rollup 的
// `inlineDynamicImports`（插件包必须单文件）会把「同包内惰性 import」提前到顶层求值，
// 所以只能拆成独立文件 —— 完整理由见 `./live2d.ts` 的文件头。
//
// 这一份的对外面就一个 `mountLive2D`（接口类型在 `./live2d.ts`，调用方不必被拖进 pixi）。

import { convertFileSrc } from "@tauri-apps/api/core";
import { Application } from "pixi.js";
import { Live2DModel } from "pixi-live2d-display-lipsyncpatch/cubism4";
import {
  assertCubism4,
  dirName,
  joinPath,
  readModelJson,
  rewriteRefs,
  type Live2DHandle,
  type MountOptions,
} from "./live2d";

/** 建画布 + 载模型，返回一个可停可毁的句柄。失败时抛错，调用方负责把消息显示出来。 */
export async function mountLive2D(host: HTMLElement, opts: MountOptions): Promise<Live2DHandle> {
  const modelPath = opts.modelPath.trim();
  if (!modelPath) throw new Error("没有模型路径");

  const json = await readModelJson(modelPath);
  assertCubism4(json);

  // 引用**必须自己改写**：asset 协议把整条绝对路径 percent-encode 进 URL 最后一段，
  // `new URL(相对引用, convertFileSrc(模型路径))` 会落到协议根、不是模型目录
  //（见 docs/agent-feature-backlog.md 的 L1 ①）。
  const modelDir = dirName(modelPath);
  const settings = rewriteRefs(json, rel => convertFileSrc(joinPath(modelDir, rel)));
  // 库的 `ModelSettings` 构造器**强制要求** `url` 是字符串（只用于 name 推断与兜底解析），
  // 给原始 model3.json 的绝对 URL 即可 —— 引用已经全部改写过了。
  (settings as { url?: string }).url = convertFileSrc(modelPath);

  const app = new Application({
    width: Math.max(1, host.clientWidth),
    height: Math.max(1, host.clientHeight),
    backgroundAlpha: 0,
    antialias: true,
    resolution: window.devicePixelRatio || 1,
    autoDensity: true,
    // 独占 ticker：窗口隐藏时要**只停自己这一个**，不能去停 `Ticker.shared`
    sharedTicker: false,
    autoStart: true,
  });
  const canvas = app.view as HTMLCanvasElement;
  canvas.style.display = "block";
  host.appendChild(canvas);

  let model: Live2DModel;
  try {
    model = await Live2DModel.from(settings as never, {
      ticker: app.ticker,
      // 桌宠窗的左键已经归「拖窗」（`startDragging`）⇒ 指针互动本来就用不上；
      // 关掉它也省掉对 pixi 事件系统的一层依赖（模型仍会自己播 Idle 动作）。
      autoHitTest: false,
      autoFocus: false,
    });
  } catch (e) {
    app.destroy(true, { children: true, texture: true, baseTexture: true });
    throw e instanceof Error ? e : new Error(String(e));
  }
  app.stage.addChild(model as never);

  // 记录**未缩放**的设计尺寸：`model.width` 会随 scale 变，fit() 里不能反着读它
  const srcW = model.width || 1;
  const srcH = model.height || 1;
  let scale = opts.scale;

  const fit = () => {
    const w = Math.max(1, host.clientWidth);
    const h = Math.max(1, host.clientHeight);
    app.renderer.resize(w, h);
    const base = Math.min(w / srcW, h / srcH);
    model.scale.set(base * scale);
    model.anchor.set(0.5, 1); // 底边中心对齐 —— 桌宠站在窗口下沿
    model.position.set(w / 2, h);
  };
  fit();

  const ro = new ResizeObserver(() => fit());
  ro.observe(host);

  return {
    setScale(next: number) {
      scale = next;
      fit();
    },
    setRunning(on: boolean) {
      if (on) app.start();
      else app.stop();
    },
    destroy() {
      ro.disconnect();
      app.destroy(true, { children: true, texture: true, baseTexture: true });
    },
  };
}
